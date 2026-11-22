// Web view frames (205 §4.4): the payload of a `web:<id>` pane leaf, in memory like the pane tree. `url` is what was
// last asked for (the tool, the address field), `current` what the page reports (`web-view://state`).

import { create } from 'zustand'
import { openExternal } from '@/lib/platform'
import * as native from '@/lib/web-view-native'
import { supports } from '@/rpc/discovery'
import { useConnection } from '@/store/connection'

export type WebView = {
  id: string
  /** The session whose `show_web` opened it; `''` in a pop-out window, which knows only the id. */
  session: string
  url: string
  title: string
  current: string
  loading: boolean
  /** Popped out of this window (205 D6): the card focuses that window instead of placing a tile. */
  popped?: boolean
  /** 209 §7, phone: hidden in the pill above the composer, its native view kept (page, scroll, form input, history). */
  minimised?: boolean
  /** When it last went full screen or into the pill (a counter): the pill shows the latest. */
  seen?: number
}

type WebViewsStore = {
  views: Record<string, WebView>
  /** 209 §7: the frame full screen on a phone (`PhoneWebView`), or none. */
  front: string | null
  /** Add the frame, or (same id: same session and origin, 205 D4) point the existing one at `url` and `title`. */
  open(view: Pick<WebView, 'id' | 'session' | 'url' | 'title'>): void
  /** Forget the frame and destroy its native web view. */
  close(id: string): void
  patch(id: string, p: Partial<Omit<WebView, 'id'>>): void
  /** 209 §7: frame `id` full screen on the phone; one that was there goes to the pill. */
  show(id: string): void
  /** 209 §7: frame `id` into the pill: its native view is hidden (`web_view_visible(false)`), never destroyed. */
  minimise(id: string): void
}

/** Orders `seen`. */
let tick = 0

const WEB = 'web:'
export const webLeaf = (id: string): string => `${WEB}${id}`
/** A pane leaf holding a web frame, not a session; check before an RPC. */
export const isWeb = (leaf: string | null | undefined): leaf is string => leaf?.startsWith(WEB) ?? false
export const webId = (leaf: string): string => leaf.slice(WEB.length)

export const useWebViews = create<WebViewsStore>()((set, get) => ({
  views: {},
  front: null,
  open({ id, session, url, title }) {
    const old = get().views[id]
    set({ views: { ...get().views, [id]: { loading: false, ...old, id, session: session || old?.session || '', url, title, current: url } } })
  },
  close(id) {
    placed.delete(id)
    if (get().front === id) set({ front: null })
    const old = get().views[id]
    if (!old) return
    const { [id]: _, ...rest } = get().views
    set({ views: rest })
    // 209 P3: the engine forgets the tile (no longer listed or attachable). A pop-out window knows no session.
    if (old.session) sendState({ session: old.session, view: webLeaf(id), url: old.current, title: old.title, loading: false, closed: true })
    void native.close(id).catch((e: unknown) => console.warn('web_view_close', e))
  },
  patch(id, p) {
    const old = get().views[id]
    if (old) set({ views: { ...get().views, [id]: { ...old, ...p } } })
  },
  show(id) {
    const { views, front } = get()
    const view = views[id]
    if (!view) return
    const next = { ...views, [id]: { ...view, minimised: false, seen: ++tick } }
    const prev = front !== null && front !== id ? views[front] : undefined
    if (prev) next[prev.id] = { ...prev, minimised: true, seen: ++tick }
    set({ views: next, front: id })
  },
  minimise(id) {
    const { views, front } = get()
    const old = views[id]
    if (!old) return
    set({ views: { ...views, [id]: { ...old, minimised: true, seen: ++tick } }, front: front === id ? null : front })
    void native.visible(id, false).catch((e: unknown) => console.warn('web_view_visible', e))
  },
}))

/** 209 §7: what the pill above the composer shows: the latest minimised frame, how many there are, and all of them,
 * latest first; `null` when none is minimised. A popped-out frame lives in its window, never in the pill. */
export function pillOf(views: Record<string, WebView>): { latest: WebView; count: number; list: WebView[] } | null {
  const list = Object.values(views)
    .filter((v) => v.minimised && !v.popped)
    .sort((a, b) => (b.seen ?? 0) - (a.seen ?? 0))
  const latest = list[0]
  return latest ? { latest, count: list.length, list } : null
}

/** How a `show_web` (or a card's Open) reaches the screen (209 D11, §7). A tablet or desktop tiles it beside its chat
 * when that chat is on screen (205 D5), else it waits for its card. A phone with native views opens it full screen,
 * except while the user is writing (the composer has focus or holds a draft) or is looking at another chat: then it
 * arrives as the pill, never over the screen. A phone without native views (a browser) keeps the card. */
export type Arrival = 'tile' | 'full' | 'pill' | 'card'
export function arrival(o: { phone: boolean; native: boolean; visible: boolean; writing: boolean }): Arrival {
  if (!o.phone) return o.visible ? 'tile' : 'card'
  if (!o.native) return 'card'
  return o.visible && !o.writing ? 'full' : 'pill'
}

/** Every frame closed, native views included: the engine they belong to was left. */
export function closeAllWebViews(): void {
  for (const id of Object.keys(useWebViews.getState().views)) useWebViews.getState().close(id)
}

/** An `http:`/`https:` address with a host, or `null`. A bare host (`localhost:3000`, `example.com/x`) gets `http://`
 * (205 §4.1); any other scheme (`javascript:`, `file:`, `data:`, …) is refused. */
export function httpUrl(input: string): string | null {
  const s = input.trim()
  // `name:` not followed by a port is a scheme; `localhost:3000` is a host.
  const schemed = /^[a-z][a-z0-9+.-]*:/i.test(s) && !/^[^:/?#]+:\d+(?:[/?#]|$)/.test(s)
  let url: URL
  try {
    url = new URL(schemed ? s : `http://${s}`)
  } catch {
    return null
  }
  return (url.protocol === 'http:' || url.protocol === 'https:') && url.hostname !== '' ? (schemed ? s : `http://${s}`) : null
}

/** Open `url` in the system browser, if it is an http(s) address (`httpUrl`); anything else is dropped. */
export function openInBrowser(url: string): void {
  const href = httpUrl(url)
  if (href) openExternal(href)
}

/** 209 D11, §7: below this tile width (logical px) the toolbar squeezes the address field to a few characters (about
 * 440 px on an iPad in portrait with the sidebar open). The contract's figure; §11 Q5 asks for a measurement. */
export const WEB_TILE_MIN_WIDTH = 420

/**
 * 209 D11, §7: collapse the sidebar as a web tile is placed? Only on iOS (the iPad: a phone is one pane at a time and
 * its sidebar is a sheet), only while the sidebar is open inline, and only when the tile came out narrower than
 * `WEB_TILE_MIN_WIDTH`. Nothing reopens it.
 */
export function collapseSidebarFor(tile: { width: number; sidebarOpen: boolean; phone: boolean; ios: boolean }): boolean {
  return tile.ios && !tile.phone && tile.sidebarOpen && tile.width > 0 && tile.width < WEB_TILE_MIN_WIDTH
}

/** Frames whose placement already had its sidebar decision (209 D11): one per tile, so a sidebar the user reopens stays
 * open across remounts; a frame closed and opened again decides again. */
const placed = new Set<string>()

/**
 * 209 D11: frame `id` was placed (shown in this window) at `tile.width`: collapse the sidebar? Decided once per frame,
 * and only from a real measurement: a width of 0 (a pane not laid out yet, a tile mounted hidden) decides nothing.
 */
export function placeTile(id: string, tile: { width: number; sidebarOpen: boolean; phone: boolean; ios: boolean }): boolean {
  if (placed.has(id) || tile.width <= 0) return false
  placed.add(id)
  return collapseSidebarFor(tile)
}

/** One tile's state for the engine (209 D9): the params of `web_view_state`, `view` as `web:<id>`; `closed` once, when
 * the tile is closed. */
export type WebViewStateReport = { session: string; view: string; url: string; title: string; loading: boolean; closed?: boolean }

/** 209 D9: at most one `web_view_state` per view in this many ms (209 §4.5). */
export const STATE_EVERY_MS = 500

/**
 * 209 D9: a view's state goes out only when it differs from what was last delivered, at most once per
 * `STATE_EVERY_MS` per view; a state arriving inside the window waits for its end, replaced by any newer one, so the
 * last state of a burst (`loading: false`) always goes. `send` answers whether it delivered (else the next change retries).
 */
export function stateForwarder(send: (r: WebViewStateReport) => boolean) {
  type Slot = { sent: string; at: number; next: WebViewStateReport | null; timer?: ReturnType<typeof setTimeout> }
  const slots = new Map<string, Slot>()
  const deliver = (s: Slot, r: WebViewStateReport) => {
    const key = JSON.stringify([r.session, r.url, r.title, r.loading])
    if (key === s.sent || !send(r)) return
    s.sent = key
    s.at = Date.now()
  }
  return {
    push(r: WebViewStateReport) {
      const s = slots.get(r.view) ?? { sent: '', at: Number.NEGATIVE_INFINITY, next: null }
      slots.set(r.view, s)
      if (s.timer !== undefined) {
        s.next = r
        return
      }
      const wait = s.at + STATE_EVERY_MS - Date.now()
      if (wait <= 0) return deliver(s, r)
      s.next = r
      s.timer = setTimeout(() => {
        s.timer = undefined
        const next = s.next
        s.next = null
        if (next) deliver(s, next)
      }, wait)
    },
    /** The view is gone: forget its slot, drop a waiting state. */
    drop(view: string) {
      clearTimeout(slots.get(view)?.timer)
      slots.delete(view)
    },
    stop() {
      for (const s of slots.values()) clearTimeout(s.timer)
      slots.clear()
    },
  }
}

/** `web_view_state` to the connected engine when it has the method (never a legacy engine); `false` when not sent. */
function sendState(r: WebViewStateReport): boolean {
  const { api, state, discovery } = useConnection.getState()
  if (!api || state !== 'open' || discovery.phase === 'legacy' || !supports(discovery, { methods: ['web_view_state'] })) return false
  const sent = r.closed
    ? api.web_view_state(r.session, r.view, r.url, r.title, r.loading, true)
    : api.web_view_state(r.session, r.view, r.url, r.title, r.loading)
  void sent.catch(() => undefined)
  return true
}

/**
 * 205 §4.4, the main window's one listener set: `web-view://state` keeps a frame's address, title and spinner current
 * wherever it is shown (a tile, a pop-out, nowhere on screen) and reports it to the engine for the session that opened
 * it (209 D9, `stateForwarder`); `web-view://closed` (the view destroyed, its pop-out window closed) clears `popped`,
 * so the card places a tile again. No tile opens by itself.
 */
export function installWebViewEvents(): () => void {
  let gone = false
  const offs: (() => void)[] = []
  const keep = (p: Promise<() => void>) => void p.then((off) => (gone ? off() : offs.push(off)))
  const forward = stateForwarder(sendState)
  keep(
    native.onState((s) => {
      const id = native.labelId(s.label)
      const old = useWebViews.getState().views[id]
      if (!old) return
      const title = s.title || old.title
      // A view with no URL yet (a load that never committed, as a LAN host that does not answer) reports `''`: it is
      // still at the address it was asked for, which the field keeps showing.
      const url = s.url || old.current
      useWebViews.getState().patch(id, { current: url, title, loading: s.loading })
      if (old.session) forward.push({ session: old.session, view: webLeaf(id), url, title, loading: s.loading })
    }),
  )
  keep(
    native.onClosed((s) => {
      const id = native.labelId(s.label)
      forward.drop(webLeaf(id))
      useWebViews.getState().patch(id, { popped: false, loading: false })
    }),
  )
  return () => {
    gone = true
    forward.stop()
    for (const off of offs) off()
  }
}

/** `localhost`, `*.localhost`, `127.0.0.0/8`, `::1` and `0.0.0.0`: an address that names the engine's machine (205 §4.4, last bullet). */
export function isLoopback(url: string): boolean {
  let host: string
  try {
    host = new URL(url).hostname.toLowerCase()
  } catch {
    return false
  }
  return host === 'localhost' || host.endsWith('.localhost') || /^127\.\d+\.\d+\.\d+$/.test(host) || host === '[::1]' || host === '0.0.0.0'
}
