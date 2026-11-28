import { useEffect, useRef } from 'react'
import { cn } from '@/lib/utils'
import { DeckView } from './DeckView'
import type { DeckLiveState } from './useDeckLive'

export type DeckCardBodyProps = {
  deckId: string
  section?: string
  snapshotId?: string
  /** True while the whiteboard card is entered/focused for interaction. */
  interactive?: boolean
  /** Optional pre-supplied live state for deterministic tests. */
  liveState?: DeckLiveState
}

/**
 * Live whiteboard card renderer for `CardKind::Deck`.
 *
 * Renders the EXACT same collaborative `DeckView` (and live `useDeckLive` subscription /
 * snapshot data) as `/decks/$id`, strictly by reference: streaming frames and snapshot
 * repaints update React state inside this card ONLY and NEVER mutate the whiteboard
 * Y.Doc, create card versions, or resize/move the card.
 */
export default function DeckCardBody({
  deckId,
  section,
  snapshotId,
  interactive = false,
  liveState,
}: DeckCardBodyProps) {
  const hostRef = useRef<HTMLDivElement>(null)

  // Canvas.tsx registers a native `window.addEventListener('keydown', ...)` for Enter/Delete/Backspace.
  // A native listener here stops non-Escape keys before they bubble to `window`, while letting Escape exit the card.
  useEffect(() => {
    const el = hostRef.current
    if (!el || !interactive) return
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') e.stopPropagation()
    }
    el.addEventListener('keydown', onKeyDown)
    return () => el.removeEventListener('keydown', onKeyDown)
  }, [interactive])

  return (
    <div
      ref={hostRef}
      className={cn('absolute inset-0 overflow-hidden', interactive && 'nodrag nopan nowheel')}
      data-deck-card={deckId}
      data-deck-interactive={interactive || undefined}
    >
      <DeckView
        deckId={deckId}
        section={section}
        snapshotId={snapshotId}
        embedded
        liveState={liveState}
      />
    </div>
  )
}
