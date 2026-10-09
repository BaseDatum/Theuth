import { createContext, useContext } from 'react'
import { create } from 'zustand'
import { useNotebookChats } from '@/components/notebooks/useNotebook'
import type { DocSummary } from '@/rpc/bindings/DocSummary'

/**
 * 144 §8: provided by a notebook's chat column around its transcript so each assistant message can offer
 * **Save answer as note** / **Promote into "<note>" ▾**. `notes` = the notebook's notes (titles for the menu);
 * `onChanged` = the Notes column refreshes after a save/promote. `null` outside a notebook.
 */
export type NotebookChat = { notebook: string; notes: readonly DocSummary[]; onChanged(): void }

export const NotebookContext = createContext<NotebookChat | null>(null)

export const useNotebookChat = (): NotebookChat | null => useContext(NotebookContext)

// 144 §8: the server lists a notebook's chats; this device remembers only which thread is open per notebook.
const KEY = 'notebook-chat'
type Saved = Record<string, string | null | undefined>

function load(): Saved {
  try {
    return JSON.parse(globalThis.localStorage?.getItem(KEY) ?? '{}') as Saved
  } catch {
    return {}
  }
}

export const useOpenThread = create<{ open: Saved; set(notebook: string, session: string | null): void }>()((set, get) => ({
  open: load(),
  set(notebook, session) {
    const open = { ...get().open, [notebook]: session }
    localStorage.setItem(KEY, JSON.stringify(open))
    set({ open })
  },
}))

/**
 * 144 wave 2 §5.1: the notebook chat's current session — what the dial writes to and the estimate reads. The
 * sources and notes columns are the chat column's siblings (not its children), so they derive it the same way the
 * chat column does: the remembered thread, else the newest chat; `null` = a new thread not yet started.
 */
export function useNotebookSession(notebook: string): string | null {
  const remembered = useOpenThread((s) => s.open[notebook])
  const chats = useNotebookChats(notebook).data
  return remembered === undefined ? (chats?.[0]?.id ?? null) : remembered
}
