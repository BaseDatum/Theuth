// Pure state for the terminal observer (51 §7.3): which page is selected, the
// last page painted, and the guards that keep a late reply for an old
// selection from painting over the current one.

import type { Color } from '@/rpc/bindings/Color'
import type { Key } from '@/rpc/bindings/Key'
import type { PtyPage } from '@/rpc/bindings/PtyPage'
import type { PtyScreenReply } from '@/rpc/bindings/PtyScreenReply'
import type { PtyViewport } from '@/rpc/bindings/PtyViewport'

export type TerminalKey = { harness: string; session: string; job: string }
export type Viewport = PtyViewport
export type QueryKey = { key: TerminalKey; viewport: Viewport }
/** Where a reply came from: the selection at request time. */
export type Origin = { queryKey: QueryKey; epoch: number }

export type Nav = 'up' | 'down' | 'pageUp' | 'pageDown' | 'live'

export type State = {
  key: TerminalKey | null
  viewport: Viewport
  /** Bumped on every job selection; a reply carrying an older epoch is dropped. */
  epoch: number
  page: PtyPage | null
  /** Newest revision painted for `key`; `if_revision` of the next request. */
  revision: number | null
  /** Daemon epoch first seen for `key`. */
  daemonEpoch: string | null
  /** An `unchanged` echoed a different viewport: refetch without `if_revision`. */
  dirty: boolean
  unavailable: boolean
  daemonRestarted: boolean
}

export const LIVE: Viewport = { anchor: 'live' }
/** Longest `history` page the daemon serves. */
export const PAGE_MAX = 80

export const EMPTY: State = {
  key: null,
  viewport: LIVE,
  epoch: 0,
  page: null,
  revision: null,
  daemonEpoch: null,
  dirty: false,
  unavailable: false,
  daemonRestarted: false,
}

export const sameKey = (a: TerminalKey | null, b: TerminalKey | null): boolean =>
  a === b || (!!a && !!b && a.harness === b.harness && a.session === b.session && a.job === b.job)

export const sameViewport = (a: Viewport, b: Viewport): boolean =>
  a.anchor === 'live' ? b.anchor === 'live' : b.anchor === 'history' && a.offset === b.offset && a.height === b.height

/** Pick a job: new epoch, live viewport, nothing carried over. Reselecting the same job is a no-op. */
export function select(state: State, key: TerminalKey): State {
  if (sameKey(state.key, key)) return state
  return { ...EMPTY, key, epoch: state.epoch + 1 }
}

/** Fold one reply in; a reply whose `origin` is not the current selection is ignored. */
export function applyReply(state: State, reply: PtyScreenReply, origin: Origin): State {
  if (origin.epoch !== state.epoch || !sameKey(origin.queryKey.key, state.key)) return state
  if (!sameViewport(origin.queryKey.viewport, state.viewport)) return state
  switch (reply.type) {
    case 'unavailable':
      return state.unavailable ? state : { ...state, unavailable: true }
    case 'unchanged':
      if (sameViewport(reply.viewport, origin.queryKey.viewport)) return state
      return state.dirty ? state : { ...state, dirty: true }
    case 'updated': {
      const revision = Number(reply.revision)
      if (state.revision !== null && revision < state.revision) return state
      const daemonRestarted =
        state.daemonRestarted || (state.daemonEpoch !== null && reply.epoch !== null && reply.epoch !== state.daemonEpoch)
      return {
        ...state,
        page: reply,
        revision,
        daemonEpoch: reply.epoch ?? state.daemonEpoch,
        dirty: false,
        unavailable: false,
        daemonRestarted,
      }
    }
  }
}

/**
 * Move through retained history. `history.offset` is rows between the page's
 * bottom and the live screen's top; `up` from live shows the page just above
 * it, `down` past offset 0 returns to live. Clamped to what `page` says is
 * retained; without a page (or with none retained) only `live` is reachable.
 */
export function nextViewport(current: Viewport, action: Nav, page: PtyPage | null): Viewport {
  if (action === 'live' || !page || page.retained === 0) return LIVE
  const height = Math.max(1, Math.min(page.rows, PAGE_MAX))
  const up = action === 'up' || action === 'pageUp'
  if (current.anchor === 'live') return up ? { anchor: 'history', offset: 0, height } : LIVE
  const max = Math.max(0, page.retained - height)
  const step = action === 'up' || action === 'down' ? 1 : height
  const next = up ? current.offset + step : current.offset - step
  if (next < 0) return LIVE
  return { anchor: 'history', offset: Math.min(next, max), height }
}

export type Action = { type: 'select'; key: TerminalKey } | { type: 'nav'; nav: Nav } | { type: 'reply'; reply: PtyScreenReply; origin: Origin }

export function reduce(state: State, action: Action): State {
  switch (action.type) {
    case 'select':
      return select(state, action.key)
    case 'nav': {
      const viewport = nextViewport(state.viewport, action.nav, state.page)
      return sameViewport(viewport, state.viewport) ? state : { ...state, viewport, dirty: false }
    }
    case 'reply':
      return applyReply(state, action.reply, action.origin)
  }
}

/** What `pty_input` gets for one keydown (55 §4.2); `null` for keys the terminal has no use for (modifiers, IME composition, browser chords left to the browser). */
export type Input = { text: string | null; keys: Key[] }

const NAMED: Record<string, Key> = {
  Enter: 'enter',
  Tab: 'tab',
  Escape: 'escape',
  Backspace: 'backspace',
  Delete: 'delete',
  Insert: 'insert',
  ArrowUp: 'up',
  ArrowDown: 'down',
  ArrowLeft: 'left',
  ArrowRight: 'right',
  Home: 'home',
  End: 'end',
  PageUp: 'page_up',
  PageDown: 'page_down',
}

export function keyToInput(e: { key: string; ctrlKey: boolean; altKey: boolean; metaKey: boolean; shiftKey: boolean; isComposing?: boolean }): Input | null {
  if (e.isComposing || e.metaKey) return null
  if (e.key === 'Tab' && e.shiftKey) return { text: null, keys: ['shift_tab'] }
  const named = NAMED[e.key]
  if (named) return { text: null, keys: [named] }
  const f = /^F(\d{1,2})$/.exec(e.key)
  if (f) return { text: null, keys: [{ f: Number(f[1]) }] }
  if ([...e.key].length !== 1) return null
  if (e.ctrlKey) return { text: null, keys: [{ ctrl: e.key }] }
  if (e.altKey) return { text: null, keys: [{ alt: e.key }] }
  return { text: e.key, keys: [] }
}

// xterm's default palette: 16 ANSI colours, the 6×6×6 cube, 24 greys.
const ANSI = [
  '#000000', '#cd0000', '#00cd00', '#cdcd00', '#0000ee', '#cd00cd', '#00cdcd', '#e5e5e5',
  '#7f7f7f', '#ff0000', '#00ff00', '#ffff00', '#5c5cff', '#ff00ff', '#00ffff', '#ffffff',
]
const CUBE = [0, 95, 135, 175, 215, 255]

/** CSS colour for a palette index. */
export function palette(idx: number): string {
  if (idx < 16) return ANSI[idx]!
  if (idx < 232) {
    const n = idx - 16
    return `rgb(${CUBE[Math.floor(n / 36)]},${CUBE[Math.floor(n / 6) % 6]},${CUBE[n % 6]})`
  }
  const g = 8 + 10 * (idx - 232)
  return `rgb(${g},${g},${g})`
}

/** CSS colour for a wire colour; `undefined` inherits. */
export const css = (c: Color): string | undefined =>
  c.kind === 'default' ? undefined : c.kind === 'idx' ? palette(c.idx) : `rgb(${c.r},${c.g},${c.b})`
