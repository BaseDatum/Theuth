// The shell's child web views (205 §4.3): every `invoke`/`listen` for them lives here, so tests mock one module and a
// container without the native layer (a browser, the iOS shell) never reaches Tauri. Coordinates are logical px
// relative to the window's content area: the placeholder's `getBoundingClientRect()`.

import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { isDesktop } from '@/lib/platform'

/** A rectangle in logical px, viewport-relative. */
export type Rect = { x: number; y: number; width: number; height: number }
/** `web-view://state`: navigation start, load finished, title change. */
export type WebViewState = { label: string; url: string; title: string; loading: boolean }

/** Only a desktop shell hosts child web views (205 D2, D7); the iOS shell and a browser get the address card. */
export const nativeWebViews = (): boolean => isDesktop()

/** The shell's label for frame `id` (205 D3). */
export const webLabel = (id: string): string => `web-${id}`

const call = (cmd: string, args: Record<string, unknown>): Promise<void> =>
  nativeWebViews() ? invoke<void>(cmd, args) : Promise.resolve()

export const attach = (id: string, url: string, r: Rect): Promise<void> =>
  call('web_view_attach', { label: webLabel(id), url, x: r.x, y: r.y, width: r.width, height: r.height })
export const bounds = (id: string, r: Rect): Promise<void> =>
  call('web_view_bounds', { label: webLabel(id), x: r.x, y: r.y, width: r.width, height: r.height })
export const visible = (id: string, show: boolean): Promise<void> => call('web_view_visible', { label: webLabel(id), visible: show })
export const navigate = (id: string, url: string): Promise<void> => call('web_view_navigate', { label: webLabel(id), url })
export const reload = (id: string): Promise<void> => call('web_view_reload', { label: webLabel(id) })
export const history = (id: string, delta: -1 | 1): Promise<void> => call('web_view_history', { label: webLabel(id), delta })
export const close = (id: string): Promise<void> => call('web_view_close', { label: webLabel(id) })

/** `web-view://state` for every frame; a no-op outside the shell. */
export const onState = (cb: (s: WebViewState) => void): Promise<() => void> =>
  nativeWebViews() ? listen<WebViewState>('web-view://state', (e) => cb(e.payload)) : Promise.resolve(() => {})
