// 218 §8.8 / design §4.5: the reply bubble's logic. After a turn of the bound session ends, its last assistant text
// shows in a glass bubble that collapses to one line after 6 s and stays until the next send or Dismiss. It is pinned
// to the card the turn touched when that is exactly one live card, else it sits above the pill.

import type { Entry } from '@/store/sessions'
import type { Board, Id } from './canvas/model'
import type { WbTurnInfo } from './wire'

export const COLLAPSE_MS = 6_000

/** The card the bubble is pinned to, or null (above the pill). */
export function bubbleAnchor(turn: WbTurnInfo | null, board: Pick<Board, 'cards'>): Id | null {
  if (!turn || turn.undone) return null
  const touched = [...new Set(turn.cards)].filter((id) => {
    const c = board.cards[id]
    return c !== undefined && c.state !== 'archived'
  })
  return touched.length === 1 ? (touched[0] ?? null) : null
}

/** The last assistant text of the turn that just ended (after the last user row), trimmed; null when there is none. */
export function lastReply(entries: readonly Entry[]): string | null {
  for (let i = entries.length - 1; i >= 0; i--) {
    const e = entries[i]
    if (!e) continue
    if (e.kind === 'user') return null
    if (e.kind === 'assistant' && e.text.trim()) return e.text.trim()
  }
  return null
}

/** One line for the collapsed bubble (markdown markers and newlines folded). */
export function oneLine(text: string, max = 140): string {
  const flat = text.replace(/[`*_#>]+/g, '').replace(/\s+/g, ' ').trim()
  return flat.length > max ? `${flat.slice(0, max - 1)}…` : flat
}

export type BubbleTimer = { start(): void; expand(): void; dispose(): void }

/** Collapse after `ms` of being shown; `expand` (a tap) shows it whole and restarts the clock. */
export function bubbleTimer(onCollapse: (collapsed: boolean) => void, ms = COLLAPSE_MS): BubbleTimer {
  let t: ReturnType<typeof setTimeout> | null = null
  const stop = () => {
    if (t !== null) clearTimeout(t)
    t = null
  }
  const start = () => {
    stop()
    onCollapse(false)
    t = setTimeout(() => onCollapse(true), ms)
  }
  return { start, expand: start, dispose: stop }
}
