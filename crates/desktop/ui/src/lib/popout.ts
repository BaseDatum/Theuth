// A chat, an artifact, a note, a notebook, a meeting or a PTY in its own desktop window: the same bundle at the same
// route with `?window=<label>`. The label (`popout-<kind>-<id>`) is what the Tauri host keys the window on
// (focus-if-open, remembered size/position, the `popout-*` capability); in a browser it is the `window.open` name.

import { invoke } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { container } from '@/lib/platform'
import { isTauri } from '@/lib/tauri'

export type PopoutTarget =
  | { kind: 'chat'; session: string }
  | { kind: 'artifact'; callId: string }
  /** `/notes/$id`: the note's editor alone; `fresh` (a note just created) adds `?new=true` so the page selects the title. */
  | { kind: 'note'; doc: string; fresh?: true }
  /** `/notebooks/$id` (144 wave 2, Sky's ruling): the notebook's three columns alone; landscape, ~1100 × 720. */
  | { kind: 'notebook'; notebook: string }
  /** `/meetings/$id`: the control center alone. */
  | { kind: 'meeting'; meeting: string }
  /** `/meetings/$id?pane=note`: the meeting's note editor alone (`?pane` is what tells it from `meeting`). */
  | { kind: 'meeting-note'; meeting: string }
  /** `/pty/$session/$job`: one agent-started terminal alone (`components/chat/TerminalViewer.tsx`). */
  | { kind: 'pty'; session: string; job: string }
  /** `/codemap/$session/$root`: one session's CodeMap on one root alone (73 §6.9). */
  | { kind: 'codemap'; session: string; root: string }
  /** `/talk/$session`: the session's talk view alone (76 §0.12); it attaches to the running talk, never starts one. */
  | { kind: 'talk'; session: string }
  /** `/vm/$name`: one sandbox VM's desktop alone (79 §6.2); label `popout-vm-<name>`. */
  | { kind: 'vm'; name: string }

const idOf = (t: PopoutTarget): string =>
  t.kind === 'chat' || t.kind === 'talk'
    ? t.session
    : t.kind === 'artifact'
      ? t.callId
      : t.kind === 'note'
        ? t.doc
        : t.kind === 'notebook'
          ? t.notebook
          : t.kind === 'vm'
            ? t.name
            : t.kind === 'pty'
              ? `${t.session}-${t.job}`
              : t.kind === 'codemap'
                ? `${t.session}-${t.root}`
                : t.meeting

const pathOf = (t: PopoutTarget): string =>
  t.kind === 'chat' || t.kind === 'talk'
    ? `/${t.kind === 'chat' ? 'c' : 'talk'}/${encodeURIComponent(t.session)}`
    : t.kind === 'artifact'
      ? `/artifacts/${encodeURIComponent(t.callId)}`
      : t.kind === 'note'
        ? `/notes/${encodeURIComponent(t.doc)}${t.fresh ? '?new=true' : ''}`
        : t.kind === 'notebook'
          ? `/notebooks/${encodeURIComponent(t.notebook)}`
          : t.kind === 'vm'
            ? `/vm/${encodeURIComponent(t.name)}`
            : t.kind === 'pty'
              ? `/pty/${encodeURIComponent(t.session)}/${encodeURIComponent(t.job)}`
              : t.kind === 'codemap'
                ? `/codemap/${encodeURIComponent(t.session)}/${encodeURIComponent(t.root)}`
                : `/meetings/${encodeURIComponent(t.meeting)}${t.kind === 'meeting-note' ? '?pane=note' : ''}`

/** Tauri window labels allow `[A-Za-z0-9/:_-]`; ids are passed through the same filter. */
const safe = (id: string) => id.replace(/[^A-Za-z0-9_-]/g, '_')

export const popoutLabel = (t: PopoutTarget): string => `popout-${t.kind}-${safe(idOf(t))}`

export const popoutUrl = (t: PopoutTarget): string => {
  const path = pathOf(t)
  return `${path}${path.includes('?') ? '&' : '?'}window=${popoutLabel(t)}`
}

/** 143 D3: where a just-created note opens — its own window on a desktop (or a browser), the page itself on a phone or iOS. */
export const noteOpenMode = (isMobile: boolean, canPopout: boolean): 'popout' | 'navigate' => (!isMobile && canPopout ? 'popout' : 'navigate')

/** What a window at `href` shows alone, or `null` for the ordinary shell (no `?window=` or a route that has no popped form). */
export function popoutTarget(href: string): PopoutTarget | null {
  const url = new URL(href, 'http://x')
  if (!url.searchParams.get('window')) return null
  const [, head, id, sub] = url.pathname.split('/')
  if (!id) return null
  if (head === 'pty') return sub ? { kind: 'pty', session: decodeURIComponent(id), job: decodeURIComponent(sub) } : null
  if (head === 'codemap') return sub ? { kind: 'codemap', session: decodeURIComponent(id), root: decodeURIComponent(sub) } : null
  if (head === 'c') return { kind: 'chat', session: decodeURIComponent(id) }
  if (head === 'talk') return { kind: 'talk', session: decodeURIComponent(id) }
  if (head === 'vm') return { kind: 'vm', name: decodeURIComponent(id) }
  if (head === 'artifacts') return { kind: 'artifact', callId: decodeURIComponent(id) }
  if (head === 'notes') return { kind: 'note', doc: decodeURIComponent(id) }
  if (head === 'notebooks') return { kind: 'notebook', notebook: decodeURIComponent(id) }
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

/** The session a popped window belongs to (`chat`, or the PTY's owning session), else `null`: `SessionDeleted` for it closes the window. */
export const popoutSession = (t: PopoutTarget | null): string | null =>
  t !== null && (t.kind === 'chat' || t.kind === 'talk' || t.kind === 'pty' || t.kind === 'codemap') ? t.session : null

/**
 * Whether this container can open a second window at all: a desktop shell (`open_window`) or a plain browser
 * (`window.open`). The iOS shell has one webview and no window manager, so every pop-out affordance — buttons, menu
 * rows, ⌥-click, "in new window" text — is hidden there. A narrow layout is not the criterion: a narrow desktop
 * window still pops out.
 */
export const canPopout = (): boolean => container() !== 'ios'

/** 144 wave 2: a notebook's three columns want a landscape window (~1100 × 720), clamped to the screen; the Tauri
 * host sizes by label in `crates/desktop/src/commands.rs::open_window`, a browser takes `window.open` features. */
export function popoutSize(t: PopoutTarget, screen: { availWidth: number; availHeight: number }): { width: number; height: number } | null {
  if (t.kind !== 'notebook') return null
  return { width: Math.min(1100, screen.availWidth), height: Math.min(720, screen.availHeight) }
}

/** Open `t` in its own window (or focus it); a browser gets a named `window.open`. */
export function openPopout(t: PopoutTarget): Promise<void> {
  if (isTauri()) return invoke<void>('open_window', { label: popoutLabel(t), url: popoutUrl(t) })
  const size = popoutSize(t, window.screen)
  if (size) window.open(popoutUrl(t), popoutLabel(t), `width=${size.width},height=${size.height}`)
  else window.open(popoutUrl(t), popoutLabel(t))
  return Promise.resolve()
}

/** Close this popped window: what it showed is gone (its chat, meeting or note was deleted). */
export function closePopout(): void {
  if (isTauri()) void getCurrentWindow().close()
  else window.close()
}
