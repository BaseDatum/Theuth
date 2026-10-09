import { createFileRoute } from '@tanstack/react-router'
import { NotePage } from '@/components/notes/NotePage'

/**
 * `/notes/$id` (59 §7.3): one note in BlockNote with its chat panel; `$id` is the DocId. `?new` = just created: the title gets focus.
 * `?nb` (144 §4, Sky's ruling) = the notebook it was opened from: the page keeps a way back to it at every width.
 */
export const Route = createFileRoute('/notes_/$id')({
  validateSearch: (s: Record<string, unknown>): { new?: true; nb?: string } => ({
    ...(s.new === true ? { new: true as const } : {}),
    ...(typeof s.nb === 'string' && s.nb ? { nb: s.nb } : {}),
  }),
  component: NotePage,
})
