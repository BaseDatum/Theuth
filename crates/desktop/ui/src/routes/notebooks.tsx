import { createFileRoute } from '@tanstack/react-router'
import { NotebooksList } from '@/components/notebooks/NotebooksList'

/** `/notebooks` (144 §12.3): the Library's notebooks list. `?archived` shows the archived ones. */
export const Route = createFileRoute('/notebooks')({
  validateSearch: (s: Record<string, unknown>): { archived?: true } => (s.archived === true ? { archived: true } : {}),
  component: NotebooksList,
})
