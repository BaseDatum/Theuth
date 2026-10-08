import { createContext, useContext } from 'react'
import type { DocSummary } from '@/rpc/bindings/DocSummary'

/**
 * 144 §8: provided by a notebook's chat column around its transcript so each assistant message can offer
 * **Save answer as note** / **Promote into "<note>" ▾**. `notes` = the notebook's notes (titles for the menu);
 * `onChanged` = the Notes column refreshes after a save/promote. `null` outside a notebook.
 */
export type NotebookChat = { notebook: string; notes: readonly DocSummary[]; onChanged(): void }

export const NotebookContext = createContext<NotebookChat | null>(null)

export const useNotebookChat = (): NotebookChat | null => useContext(NotebookContext)
