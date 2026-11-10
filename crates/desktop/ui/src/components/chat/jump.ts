// "jump to message" (Sky, 2026-11-10): a send card asks; the transcript that renders the session scrolls, through its
// own virtualiser. Nothing here belongs to the card, so the card's row leaving the virtual window mid-jump cannot stop it.
import { toast } from 'sonner'
import { create } from 'zustand'
import { disclosureKey, useToolRuns } from '@/lib/tool-runs'
import type { Entry } from '@/store/sessions'
import type { Row } from './rows'

type JumpStore = {
  /** The session whose transcript should scroll, and the entry to bring into view; `nonce` makes each click a new request. */
  session: string | null
  entryId: string | null
  nonce: number
  request(session: string, entryId: string): void
  clear(): void
}

export const useTranscriptJump = create<JumpStore>()((set) => ({
  session: null,
  entryId: null,
  nonce: 0,
  request: (session, entryId) => set((s) => ({ session, entryId, nonce: s.nonce + 1 })),
  clear: () => set({ session: null, entryId: null }),
}))

export const NOT_LOADED = "The answer isn't loaded in this view"

/** The rendered row holding `entryId` (a plain entry, or a member of a folded run), `-1` when no row does. */
export function jumpIndex(rows: readonly Row[], entryId: string): number {
  return rows.findIndex((r) => ('entry' in r ? r.entry.id === entryId : 'run' in r && r.run.some((e) => e.id === entryId)))
}

/**
 * Consume the pending request for `session`, once: the store is cleared before anything else, so a second transcript
 * of the same session (or a re-render) never repeats it. A folded peer group's line is opened first, so its card
 * mounts with the row. `null`: nothing pending here, or no row holds the entry (the toast says so).
 */
export function takeJump(session: string, rows: readonly Row[]): { index: number; entry: Entry } | null {
  const { session: target, entryId, clear } = useTranscriptJump.getState()
  if (target !== session || entryId === null) return null
  clear()
  const index = jumpIndex(rows, entryId)
  const row = rows[index]
  const entry = row && ('entry' in row ? row.entry : 'run' in row ? row.run.find((e) => e.id === entryId) : undefined)
  if (!row || !entry) {
    toast.message(NOT_LOADED)
    return null
  }
  if ('run' in row) useToolRuns.getState().setDisclosed(disclosureKey(entryId, 'line'), true)
  return { index, entry }
}

/** The reader's own input on the scroller ends the fine-tune: the position is theirs again. */
const INPUT = ['wheel', 'touchstart', 'keydown', 'pointerdown'] as const
/** How long the jumped-to card keeps its highlight (`data-jumped`, styled by the card with theme tokens). */
const FLASH_MS = 1600

/**
 * After the virtualiser's `scrollToIndex`: wait (per frame, at most ~2 s) until `card()` is mounted and the scroller has
 * held still for three frames (the library's own dynamic-size reconcile is done, or it would undo the fine-tune), then
 * centre the card and mark it `data-jumped` briefly. An attribute, not classes: React rewrites `className` when the card
 * re-renders and would drop them. Returns the cancel; the reader's input on `scroller` cancels too.
 */
export function settleOn(scroller: HTMLElement | null, card: () => Element | null): () => void {
  let frame = 0
  let still = 0
  let tries = 0
  let last = scroller?.scrollTop
  const cancel = () => {
    cancelAnimationFrame(frame)
    for (const type of INPUT) scroller?.removeEventListener(type, cancel)
  }
  for (const type of INPUT) scroller?.addEventListener(type, cancel, { passive: true })
  const tick = () => {
    const el = card()
    const top = scroller?.scrollTop
    still = top === last ? still + 1 : 0
    last = top
    if (el && still >= 3) {
      el.scrollIntoView({ block: 'center' })
      el.setAttribute('data-jumped', '')
      setTimeout(() => el.removeAttribute('data-jumped'), FLASH_MS)
      return cancel()
    }
    if (++tries > 120) return cancel()
    frame = requestAnimationFrame(tick)
  }
  frame = requestAnimationFrame(tick)
  return cancel
}
