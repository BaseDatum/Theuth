import { createContext, useContext } from 'react'
import { create } from 'zustand'
import { useQueryClient } from '@tanstack/react-query'
import { useNotebook, useNotebookChats } from '@/components/notebooks/useNotebook'
import { remembered, useContextDefaults } from '@/lib/notebook-context'
import { type ContextApi, notebookKeys, useContextApi } from '@/lib/notebooks-rpc'
import { useApi } from '@/lib/rpc'
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

/** §5.1 W-g: this device's remembered dial positions become `session`'s rows (a new thread, however it was created). */
export const applyContextDefaults = (ctx: ContextApi, session: string, notebook: string): Promise<unknown> =>
  Promise.all(remembered(useContextDefaults.getState().defaults, notebook).map((r) => ctx.notebook_context_set(session, r.item, r.kind, r.level)))

/**
 * 144 wave 2 §5.1 (Z2): a dial touched before the first message needs a thread to write to, or the first turn would
 * run on the default rule. Creates the notebook-bound durable session (`create_session { notebook }` — the same
 * binding the composer's first send gets through `session_set_notebook`), titles it like the composer does, remembers
 * it as the open thread (the composer re-keys onto it, so the first message lands there), writes the device defaults
 * to it and refreshes the chat list. Answers the existing thread when there is one.
 */
export function useEnsureNotebookSession(notebook: string): () => Promise<string> {
  const api = useApi()
  const ctx = useContextApi()
  const qc = useQueryClient()
  const session = useNotebookSession(notebook)
  const title = useNotebook(notebook).data?.doc.title
  const remember = useOpenThread((s) => s.set)
  return async () => {
    if (session) return session
    const open = await api.create_session({ retention: 'durable', project: null, notebook, view: null, cwd: null, model: null, effort: null, parent_session: null, parent_engine: null, harness: null })
    remember(notebook, open.session)
    if (title) await api.rename_session(open.session, title)
    await applyContextDefaults(ctx, open.session, notebook)
    void qc.invalidateQueries({ queryKey: notebookKeys.chats(notebook) })
    return open.session
  }
}
