import { createFileRoute } from '@tanstack/react-router'
import { DecksList } from '@/components/decks/DecksList'
import { requirements } from '@/components/decks/requirements'
import { withCapability } from '@/rpc/capabilities'

/** `/decks`: collaborative Decks list with New deck and Workspace Pulse (Example). */
export const Route = createFileRoute('/decks')({
  component: withCapability(DecksList, requirements),
})
