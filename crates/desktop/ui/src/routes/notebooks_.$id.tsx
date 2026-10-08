import { createFileRoute } from '@tanstack/react-router'
import { NotebookPage } from '@/components/notebooks/NotebookPage'

/** `/notebooks/$id` (144 §12.1): one notebook — sources | chat | notes. `?new` = just created: the title gets focus; `?tab` = the phone's open column. */
export const Route = createFileRoute('/notebooks_/$id')({
  validateSearch: (s: Record<string, unknown>): { new?: true; tab?: 'sources' | 'chat' | 'notes' } => ({
    ...(s.new === true ? { new: true as const } : {}),
    ...(s.tab === 'sources' || s.tab === 'chat' || s.tab === 'notes' ? { tab: s.tab } : {}),
  }),
  component: NotebookPage,
})
