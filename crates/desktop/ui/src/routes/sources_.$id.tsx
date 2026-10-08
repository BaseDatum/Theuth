import { createFileRoute } from '@tanstack/react-router'
import { SourcePage } from '@/components/sources/SourcePage'

/** `/sources/$id` (144 §12.2): a read-only source. `?nb` = the notebook it was opened from (the back link); `?tab` = Content · Insights · Details. */
export const Route = createFileRoute('/sources_/$id')({
  validateSearch: (s: Record<string, unknown>): { nb?: string; tab?: 'content' | 'insights' | 'details' } => ({
    ...(typeof s.nb === 'string' && s.nb ? { nb: s.nb } : {}),
    ...(s.tab === 'content' || s.tab === 'insights' || s.tab === 'details' ? { tab: s.tab } : {}),
  }),
  component: SourcePage,
})
