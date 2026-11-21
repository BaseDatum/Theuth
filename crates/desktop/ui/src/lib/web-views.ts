// Web view frames (205 §4.4): the payload of a `web:<id>` pane leaf, in memory like the pane tree. `url` is what was
// last asked for (the tool, the address field), `current` what the page reports (`web-view://state`).

import { create } from 'zustand'
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

/** What the address field sends: `localhost:3000` and `example.com/x` get `http://` (205 §4.1); a scheme is kept. */
export const normalizeUrl = (input: string): string => {
  const s = input.trim()
  return /^[a-z][a-z0-9+.-]*:\/\//i.test(s) ? s : `http://${s}`
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
