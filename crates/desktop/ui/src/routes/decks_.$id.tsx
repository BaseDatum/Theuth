import { createFileRoute } from '@tanstack/react-router'
import { DeckView } from '@/components/decks/DeckView'
import { requirements } from '@/components/decks/requirements'
import { withCapability } from '@/rpc/capabilities'

/** `/decks/$id`: full-page collaborative Deck view with optional `?section` or `?snapshot`. */
export const Route = createFileRoute('/decks_/$id')({
  validateSearch: (s: Record<string, unknown>): { section?: string; snapshot?: string } => ({
    section: typeof s.section === 'string' ? s.section : undefined,
    snapshot: typeof s.snapshot === 'string' ? s.snapshot : undefined,
  }),
  component: withCapability(Page, requirements),
})

function Page() {
  const { id } = Route.useParams()
  const { section, snapshot } = Route.useSearch()
  return <DeckView key={id} deckId={id} section={section} snapshotId={snapshot} />
}
