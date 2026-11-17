import type { Entry } from '@/store/sessions'

/** 30 §8: a later assistant reply before the next human message carries the fix for any fence broken in entry `id`. */
export function hasLaterAssistant(entries: readonly Entry[], id: string): boolean {
  for (let i = entries.findIndex((e) => e.id === id) + 1; i > 0 && i < entries.length; i++) {
    const kind = entries[i]?.kind
    if (kind === 'assistant') return true
    if (kind === 'user') return false
  }
  return false
}

/**
 * Whether the turn that wrote entry `id` still runs (fence-skel D2): the session runs and the entry is of its running
 * turn (`turn === turnSeq`), so a reply of an earlier turn reads as settled while a later turn runs. An entry whose turn
 * is unknown (or a session whose running turn is) counts as running only in the live tail, after the last turn-opening
 * `user` entry.
 */
export function ownTurnRunning(entries: readonly Entry[], id: string, running: boolean, turnSeq: number | undefined): boolean {
  if (!running) return false
  const at = entries.findIndex((e) => e.id === id)
  const entry = entries[at]
  if (!entry) return false
  const turn = entry.kind === 'assistant' ? entry.turn : undefined
  if (turn !== undefined && turnSeq !== undefined) return turn === turnSeq
  return at > entries.findLastIndex((e) => e.kind === 'user' && e.turn !== undefined)
}
