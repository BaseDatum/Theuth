// A chat or an artifact in its own desktop window: the same bundle at the same route with `?window=<label>`.
// The label (`popout-chat-<session>` / `popout-artifact-<callId>`) is what the Tauri host keys the window on
// (focus-if-open, remembered size/position, the `popout-*` capability); in a browser it is the `window.open` name.

import { invoke } from '@tauri-apps/api/core'
import { isTauri } from '@/lib/tauri'

export type PopoutTarget = { kind: 'chat'; session: string } | { kind: 'artifact'; callId: string }

/** Tauri window labels allow `[A-Za-z0-9/:_-]`; ids are passed through the same filter. */
const safe = (id: string) => id.replace(/[^A-Za-z0-9_-]/g, '_')

export const popoutLabel = (t: PopoutTarget): string =>
  t.kind === 'chat' ? `popout-chat-${safe(t.session)}` : `popout-artifact-${safe(t.callId)}`

export const popoutUrl = (t: PopoutTarget): string =>
  `${t.kind === 'chat' ? `/c/${encodeURIComponent(t.session)}` : `/artifacts/${encodeURIComponent(t.callId)}`}?window=${popoutLabel(t)}`

/** What a window at `href` shows alone, or `null` for the ordinary shell (no `?window=` or a route that has no popped form). */
export function popoutTarget(href: string): PopoutTarget | null {
  const url = new URL(href, 'http://x')
  if (!url.searchParams.get('window')) return null
  const [, head, id] = url.pathname.split('/')
  if (!id) return null
  if (head === 'c') return { kind: 'chat', session: decodeURIComponent(id) }
  if (head === 'artifacts') return { kind: 'artifact', callId: decodeURIComponent(id) }
  return null
}

/** Read once: the route may drop the query on its first navigation. */
export const POPOUT: PopoutTarget | null = typeof location === 'undefined' ? null : popoutTarget(location.href)

export const isPopout = (): boolean => POPOUT !== null

/** Open `t` in its own window (or focus it); a browser gets a named `window.open`. */
export function openPopout(t: PopoutTarget): Promise<void> {
  if (isTauri()) return invoke<void>('open_window', { label: popoutLabel(t), url: popoutUrl(t) })
  window.open(popoutUrl(t), popoutLabel(t))
  return Promise.resolve()
}
