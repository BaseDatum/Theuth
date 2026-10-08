// 144 §13.3: the notebook RPCs are `Api` methods (`rpc/api.ts`, names checked against `bindings/methods.json`).
// This module keeps the query keys and the one hook notebook components share; a guest page without the app's
// connection binds a client of its own with `notebooksApi`.
import { type Api, makeApi } from '@/rpc/api'
import type { RpcClient } from '@/rpc/client'
import { useApi } from '@/lib/rpc'

export type NotebooksApi = Pick<
  Api,
  | 'notebook_list'
  | 'notebook_get'
  | 'notebook_create'
  | 'notebook_update'
  | 'notebook_delete_preview'
  | 'notebook_delete'
  | 'notebook_items_list'
  | 'notebook_item_add'
  | 'notebook_item_remove'
  | 'notebook_chats'
  | 'session_set_notebook'
  | 'source_add'
  | 'source_get'
  | 'source_list'
  | 'source_refresh'
  | 'cite_resolve'
  | 'nb_search'
  | 'save_answer_as_note'
  | 'promote_answer'
>

/** The notebook RPCs over one client (a guest's `/d` client); the server gates what a guest may call. */
export const notebooksApi = (client: RpcClient): NotebooksApi => makeApi(client)

/** The connected engine's notebook RPCs; like `useApi`, only rendered once connected. */
export const useNotebooksApi = (): NotebooksApi => useApi()

export const notebookKeys = {
  all: ['notebooks'] as const,
  list: (archived: boolean) => ['notebooks', 'list', archived] as const,
  one: (id: string) => ['notebooks', 'one', id] as const,
  items: (id: string) => ['notebooks', 'items', id] as const,
  chats: (id: string) => ['notebooks', 'chats', id] as const,
  sources: (notebook: string | null) => ['notebooks', 'sources', notebook] as const,
  source: (id: string) => ['notebooks', 'source', id] as const,
  /** The notebook document's markdown (the description, §3.2). */
  description: (id: string) => ['notebooks', 'description', id] as const,
  cite: (hrefs: readonly string[]) => ['cite', ...hrefs] as const,
}
