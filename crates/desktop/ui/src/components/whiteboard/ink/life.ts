// §2.7 ink life → what the owner sees.
import { lifeOf, type Board, type Id, type InkLife, type Stroke } from '../canvas/model'

export const SENT_OPACITY = 0.3

/**
 * Opacity of a stroke, 0 = not drawn. `shownVersion` is the version its card shows (scrubbed or current); anchored ink of
 * another version is hidden — it marks that version, and "answered ink is hidden with the version it marked".
 */
export function inkOpacity(life: InkLife, o: { showAnswered: boolean; scrubbed: boolean }): number {
  switch (life) {
    case 'fresh':
    case 'pinned':
      return 1
    case 'sent':
      return SENT_OPACITY
    case 'answered':
      return o.showAnswered || o.scrubbed ? SENT_OPACITY : 0
  }
}

export type InkLook = { opacity: number; pinned: boolean }

/** The look of one stroke on the canvas, given the card versions in view (`scrub`: card → version shown). */
export function lookOf(board: Board, stroke: Stroke, view: { showAnswered: boolean; scrub: Record<Id, number> }): InkLook {
  const life = lifeOf(board, stroke.id)
  let scrubbed = false
  if (stroke.card) {
    const card = board.cards[stroke.card]
    if (!card || card.state === 'archived') return { opacity: 0, pinned: false }
    const shown = view.scrub[card.id] ?? card.versions.length
    if ((stroke.ver ?? 0) !== shown) {
      // Ink of another version: only pinned annotations stay; the rest belongs to its version.
      if (life === 'pinned') return { opacity: 1, pinned: true }
      if (life === 'fresh' && view.scrub[card.id] === undefined && (stroke.ver ?? 0) === 0 && card.versions.length <= 1) {
        return { opacity: 1, pinned: false } // drawn on a placeholder, now filled as v1
      }
      return { opacity: 0, pinned: false }
    }
    scrubbed = view.scrub[card.id] !== undefined && view.scrub[card.id] !== card.versions.length
  }
  return { opacity: inkOpacity(life, { showAnswered: view.showAnswered, scrubbed }), pinned: life === 'pinned' }
}
