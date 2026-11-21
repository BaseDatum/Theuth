// The shell's native web views (205 §4.3, §9): every `invoke`/`listen` for them lives here, so tests mock one module
// and a container without the native layer (a browser) never reaches Tauri. Coordinates are logical px relative to the
// window's content area: the placeholder's `getBoundingClientRect()`.
//
// Tauri runs async commands concurrently, so calls for one label are serialised here on one promise chain, in issue
// order; a rejected call does not break the chain. Visibility and bounds are desired state: a call records it and the
// chain applies whatever differs from what the shell last got, so the state set while an attach was in flight is
// applied once it lands.

import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { container, isDesktop } from '@/lib/platform'

/** A rectangle in logical px, viewport-relative. */
export type Rect = { x: number; y: number; width: number; height: number }
/** `web-view://state`: navigation start, load finished, title change. */
export type WebViewState = { label: string; url: string; title: string; loading: boolean }
/** `web-view://closed`: the view was destroyed (`web_view_close`, or its window closed). */
export type WebViewClosed = { label: string }

/** A desktop shell (child webviews, 205 D2) and the iOS shell (a raw `WKWebView` over the tile, 205 §9) host web views;
 * a browser gets the address card (205 D7). */
export const nativeWebViews = (): boolean => isDesktop() || container() === 'ios'

/** The shell's label for frame `id` (205 D3). */
export const webLabel = (id: string): string => `web-${id}`
/** The id a `web-<id>` label names. */
export const labelId = (label: string): string => label.replace(/^web-/, '')

/** What this window wants of a view it hosts, and what the shell last got. `owned`: attached here, not yet hidden for good. */
type State = { owned: boolean; visible: boolean; rect: Rect; sent: { visible: boolean; rect: Rect | null } }
const states = new Map<string, State>()
const chains = new Map<string, Promise<unknown>>()

/** Run `step` after every earlier call for `label`; its result (or rejection) is the caller's, never the chain's. */
function queue<T>(label: string, step: () => Promise<T>): Promise<T> {
  const next = (chains.get(label) ?? Promise.resolve()).then(step)
  const settled = next.catch(() => undefined)
  chains.set(label, settled)
  void settled.then(() => {
    if (chains.get(label) === settled) chains.delete(label)
  })
  return next
}

const same = (a: Rect, b: Rect | null) => b !== null && a.x === b.x && a.y === b.y && a.width === b.width && a.height === b.height

/** Send what differs between `label`'s desired and sent state: hide before a move, move before a show. */
async function apply(label: string): Promise<void> {
  const s = states.get(label)
  if (!s?.owned) return
  if (!s.visible && s.sent.visible) {
    await invoke<void>('web_view_visible', { label, visible: false })
    s.sent.visible = false
  }
  if (!same(s.rect, s.sent.rect)) {
    const r = s.rect
    await invoke<void>('web_view_bounds', { label, x: r.x, y: r.y, width: r.width, height: r.height })
    s.sent.rect = r
  }
  if (s.visible && !s.sent.visible) {
    await invoke<void>('web_view_visible', { label, visible: true })
    s.sent.visible = true
  }
}

/** Create the view here at `r`, or move an existing one here and show it. Rejects `in-popout: …` when a pop-out window holds it. */
export function attach(id: string, url: string, r: Rect): Promise<void> {
  if (!nativeWebViews()) return Promise.resolve()
  const label = webLabel(id)
  const s: State = { owned: false, visible: true, rect: r, sent: { visible: false, rect: null } }
  states.set(label, s)
  return queue(label, async () => {
    const at = s.rect
    await invoke<void>('web_view_attach', { label, url, x: at.x, y: at.y, width: at.width, height: at.height })
    if (states.get(label) !== s) return
    s.owned = true
    s.sent = { visible: true, rect: at }
    await apply(label)
  })
}

export function bounds(id: string, r: Rect): Promise<void> {
  const s = states.get(webLabel(id))
  if (!nativeWebViews() || !s) return Promise.resolve()
  s.rect = r
  return queue(webLabel(id), () => apply(webLabel(id)))
}

export function visible(id: string, show: boolean): Promise<void> {
  const s = states.get(webLabel(id))
  if (!nativeWebViews() || !s) return Promise.resolve()
  s.visible = show
  return queue(webLabel(id), () => apply(webLabel(id)))
}

/** The frame left this window (unmount): hide the view, then stop driving it, so a window that now holds it is left alone. */
export function detach(id: string): Promise<void> {
  const label = webLabel(id)
  const s = states.get(label)
  if (!nativeWebViews() || !s) return Promise.resolve()
  s.visible = false
  return queue(label, async () => {
    try {
      await apply(label)
    } finally {
      if (states.get(label) === s) states.delete(label)
    }
  })
}

const call = (id: string, cmd: string, args: Record<string, unknown> = {}): Promise<void> =>
  nativeWebViews() ? queue(webLabel(id), () => invoke<void>(cmd, { label: webLabel(id), ...args })) : Promise.resolve()

/** Rejects `no web view …` when the label does not exist. */
export const navigate = (id: string, url: string): Promise<void> => call(id, 'web_view_navigate', { url })
export const reload = (id: string): Promise<void> => call(id, 'web_view_reload')
export const history = (id: string, delta: -1 | 1): Promise<void> => call(id, 'web_view_history', { delta })
/** Destroy the view, after every call issued before it. */
export function close(id: string): Promise<void> {
  const label = webLabel(id)
  if (!nativeWebViews()) return Promise.resolve()
  // The state goes with the view, in order: calls issued before the close still apply first.
  return queue(label, async () => {
    states.delete(label)
    await invoke<void>('web_view_close', { label })
  })
}

/** `web-view://state` for every frame; a no-op outside the shell. */
export const onState = (cb: (s: WebViewState) => void): Promise<() => void> =>
  nativeWebViews() ? listen<WebViewState>('web-view://state', (e) => cb(e.payload)) : Promise.resolve(() => {})
/** `web-view://closed` for every frame; a no-op outside the shell. */
export const onClosed = (cb: (s: WebViewClosed) => void): Promise<() => void> =>
  nativeWebViews() ? listen<WebViewClosed>('web-view://closed', (e) => cb(e.payload)) : Promise.resolve(() => {})
