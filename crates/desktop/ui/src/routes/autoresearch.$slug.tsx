import { createFileRoute } from '@tanstack/react-router'
import { StudyView } from '@/components/autoresearch/StudyView'

/** `/autoresearch/<slug>` (140 §13.1): the Study view. `?n=<experiment>` opens that experiment's detail sheet. */
export const Route = createFileRoute('/autoresearch/$slug')({
  validateSearch: (s: Record<string, unknown>): { n?: number } => (typeof s.n === 'number' && Number.isInteger(s.n) && s.n > 0 ? { n: s.n } : {}),
  component: StudyView,
})
