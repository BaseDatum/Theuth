// Web view frames (205 §4.4): the payload of a `web:<id>` pane leaf, in memory like the pane tree. `url` is what was
// last asked for (the tool, the address field), `current` what the page reports (`web-view://state`).

import { create } from 'zustand'
import { openExternal } from '@/lib/platform'
import * as native from '@/lib/web-view-native'

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
}

type WebViewsStore = {
  views: Record<string, WebView>
  /** Add the frame, or (same id: same session and origin, 205 D4) point the existing one at `url` and `title`. */
  open(view: Pick<WebView, 'id' | 'session' | 'url' | 'title'>): void
  /** Forget the frame and destroy its native web view. */
  close(id: string): void
  patch(id: string, p: Partial<Omit<WebView, 'id'>>): void
}

const WEB = 'web:'
export const webLeaf = (id: string): string => `${WEB}${id}`
/** A pane leaf holding a web frame, not a session; check before an RPC. */
export const isWeb = (leaf: string | null | undefined): leaf is string => leaf?.startsWith(WEB) ?? false
export const webId = (leaf: string): string => leaf.slice(WEB.length)

export const useWebViews = create<WebViewsStore>()((set, get) => ({
  views: {},
  open({ id, session, url, title }) {
    const old = get().views[id]
    set({ views: { ...get().views, [id]: { loading: false, ...old, id, session: session || old?.session || '', url, title, current: url } } })
  },
  close(id) {
    if (!(id in get().views)) return
    const { [id]: _, ...rest } = get().views
    set({ views: rest })
    void native.close(id).catch((e: unknown) => console.warn('web_view_close', e))
  },
  patch(id, p) {
    const old = get().views[id]
    if (old) set({ views: { ...get().views, [id]: { ...old, ...p } } })
  },
}))

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

/**
 * 205 §4.4, the main window's one listener set: `web-view://state` keeps a frame's address, title and spinner current
 * wherever it is shown (a tile, a pop-out, nowhere on screen); `web-view://closed` (the view destroyed, its pop-out
 * window closed) clears `popped`, so the card places a tile again. No tile opens by itself.
 */
export function installWebViewEvents(): () => void {
  let gone = false
  const offs: (() => void)[] = []
  const keep = (p: Promise<() => void>) => void p.then((off) => (gone ? off() : offs.push(off)))
  keep(
    native.onState((s) => {
      const id = native.labelId(s.label)
      const old = useWebViews.getState().views[id]
      if (old) useWebViews.getState().patch(id, { current: s.url, title: s.title || old.title, loading: s.loading })
    }),
  )
  keep(native.onClosed((s) => useWebViews.getState().patch(native.labelId(s.label), { popped: false, loading: false })))
  return () => {
    gone = true
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
