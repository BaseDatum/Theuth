import { createFileRoute } from '@tanstack/react-router'
import { NotebookPage } from '@/components/notebooks/NotebookPage'

/** `/notebooks/$id` (144 §12.1): one notebook — sources | chat | notes. `?new` = just created: the title gets focus; `?tab` = the phone's open column;
 * `?ask` (wave 2, §9) = the Ask across notebooks dialog is open (`true`, or the question `/ask <words>` typed). */
export const Route = createFileRoute('/notebooks_/$id')({
  validateSearch: (s: Record<string, unknown>): { new?: true; tab?: 'sources' | 'chat' | 'notes'; ask?: string | true } => ({
    ...(s.new === true ? { new: true as const } : {}),
    ...(s.tab === 'sources' || s.tab === 'chat' || s.tab === 'notes' ? { tab: s.tab } : {}),
    ...(s.ask === true || (typeof s.ask === 'string' && s.ask !== '') ? { ask: s.ask } : {}),
  }),
  component: NotebookPage,
})
