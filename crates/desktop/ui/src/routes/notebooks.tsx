import { createFileRoute } from '@tanstack/react-router'
import { NotebooksList } from '@/components/notebooks/NotebooksList'

/** `/notebooks` (144 §12.3): the Library's notebooks list. `?archived` shows the archived ones; `?view=ask` (wave 2, §9) the past asks;
 * `?ask` opens the Ask across notebooks dialog over all notebooks (`true`, or the question `/ask <words>` typed). */
export const Route = createFileRoute('/notebooks')({
  validateSearch: (s: Record<string, unknown>): { archived?: true; view?: 'ask'; ask?: string | true } => ({
    ...(s.archived === true ? { archived: true as const } : {}),
    ...(s.view === 'ask' ? { view: 'ask' as const } : {}),
    ...(s.ask === true || (typeof s.ask === 'string' && s.ask !== '') ? { ask: s.ask } : {}),
  }),
  component: NotebooksList,
})
