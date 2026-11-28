import { DeckView } from './DeckView'
import type { DeckLiveState } from './useDeckLive'

export type DeckCardBodyProps = {
  deckId: string
  section?: string
  snapshotId?: string
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
export default function DeckCardBody({ deckId, section, snapshotId, liveState }: DeckCardBodyProps) {
  return (
    <div className="absolute inset-0 overflow-hidden" data-deck-card={deckId}>
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
