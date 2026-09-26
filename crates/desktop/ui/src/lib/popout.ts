// A chat, an artifact, a note or a meeting in its own desktop window: the same bundle at the same route with
// `?window=<label>`. The label (`popout-<kind>-<id>`) is what the Tauri host keys the window on (focus-if-open,
// remembered size/position, the `popout-*` capability); in a browser it is the `window.open` name.

import { invoke } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { container } from '@/lib/platform'
import { isTauri } from '@/lib/tauri'

export type PopoutTarget =
  | { kind: 'chat'; session: string }
  | { kind: 'artifact'; callId: string }
  /** `/notes/$id`: the note's editor alone. */
  | { kind: 'note'; doc: string }
  /** `/meetings/$id`: the control center alone. */
  | { kind: 'meeting'; meeting: string }
  /** `/meetings/$id?pane=note`: the meeting's note editor alone (`?pane` is what tells it from `meeting`). */
  | { kind: 'meeting-note'; meeting: string }

const idOf = (t: PopoutTarget): string =>
  t.kind === 'chat' ? t.session : t.kind === 'artifact' ? t.callId : t.kind === 'note' ? t.doc : t.meeting

const pathOf = (t: PopoutTarget): string =>
  t.kind === 'chat'
    ? `/c/${encodeURIComponent(t.session)}`
    : t.kind === 'artifact'
      ? `/artifacts/${encodeURIComponent(t.callId)}`
      : t.kind === 'note'
        ? `/notes/${encodeURIComponent(t.doc)}`
        : `/meetings/${encodeURIComponent(t.meeting)}${t.kind === 'meeting-note' ? '?pane=note' : ''}`

/** Tauri window labels allow `[A-Za-z0-9/:_-]`; ids are passed through the same filter. */
const safe = (id: string) => id.replace(/[^A-Za-z0-9_-]/g, '_')

export const popoutLabel = (t: PopoutTarget): string => `popout-${t.kind}-${safe(idOf(t))}`

export const popoutUrl = (t: PopoutTarget): string => `${pathOf(t)}${t.kind === 'meeting-note' ? '&' : '?'}window=${popoutLabel(t)}`

/** What a window at `href` shows alone, or `null` for the ordinary shell (no `?window=` or a route that has no popped form). */
export function popoutTarget(href: string): PopoutTarget | null {
  const url = new URL(href, 'http://x')
  if (!url.searchParams.get('window')) return null
  const [, head, id] = url.pathname.split('/')
  if (!id) return null
  if (head === 'c') return { kind: 'chat', session: decodeURIComponent(id) }
  if (head === 'artifacts') return { kind: 'artifact', callId: decodeURIComponent(id) }
  if (head === 'notes') return { kind: 'note', doc: decodeURIComponent(id) }
  if (head === 'meetings') {
    const meeting = decodeURIComponent(id)
    return url.searchParams.get('pane') === 'note' ? { kind: 'meeting-note', meeting } : { kind: 'meeting', meeting }
  }
  return null
}

/** Read once: the route may drop the query on its first navigation. */
export const POPOUT: PopoutTarget | null = typeof location === 'undefined' ? null : popoutTarget(location.href)

export const isPopout = (): boolean => POPOUT !== null

/** The meeting a popped window is about (`meeting` or `meeting-note`), else `null`: a `Deleted` event for it closes the window. */
export const popoutMeeting = (t: PopoutTarget | null): string | null =>
  t !== null && (t.kind === 'meeting' || t.kind === 'meeting-note') ? t.meeting : null

/**
 * Whether this container can open a second window at all: a desktop shell (`open_window`) or a plain browser
 * (`window.open`). The iOS shell has one webview and no window manager, so every pop-out affordance — buttons, menu
 * rows, ⌥-click, "in new window" text — is hidden there. A narrow layout is not the criterion: a narrow desktop
 * window still pops out.
 */
export const canPopout = (): boolean => container() !== 'ios'

/** Open `t` in its own window (or focus it); a browser gets a named `window.open`. */
export function openPopout(t: PopoutTarget): Promise<void> {
  if (isTauri()) return invoke<void>('open_window', { label: popoutLabel(t), url: popoutUrl(t) })
  window.open(popoutUrl(t), popoutLabel(t))
  return Promise.resolve()
}

/** Close this popped window: what it showed is gone (its chat, meeting or note was deleted). */
export function closePopout(): void {
  if (isTauri()) void getCurrentWindow().close()
  else window.close()
}
