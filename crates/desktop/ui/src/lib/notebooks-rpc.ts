// 144 §13.3: the notebook RPCs are `Api` methods (`rpc/api.ts`, names checked against `bindings/methods.json`).
// This module keeps the query keys and the one hook notebook components share; a guest page without the app's
// connection binds a client of its own with `notebooksApi`.
// 144 wave 2 (contract §1): the dial / estimate / insight methods E1 adds are mirrored here by hand (`ContextApi`)
// until the closer puts them on `Api` — `useContextApi()` then becomes `useApi()` and the interface a `Pick`.
import { type Api, makeApi } from '@/rpc/api'
import type { ContextEstimate } from '@/rpc/bindings/ContextEstimate'
import type { ContextLevel } from '@/rpc/bindings/ContextLevel'
import type { InsightInstruction } from '@/rpc/bindings/InsightInstruction'
import type { InsightRun } from '@/rpc/bindings/InsightRun'
import type { ItemKind } from '@/rpc/bindings/ItemKind'
import type { RpcClient } from '@/rpc/client'
import { useApi } from '@/lib/rpc'
import { useConnection } from '@/store/connection'

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

/** Contract §1 methods, positional params in the contract's order; hub-only except `notebook_context_estimate`. */
export type ContextApi = {
  notebook_context_set(session: string, item: string, kind: ItemKind, level: ContextLevel): Promise<null>
  notebook_context_clear(session: string, item: string): Promise<null>
  notebook_context_set_all(session: string, notebook: string, kind: ItemKind | null, level: ContextLevel): Promise<null>
  notebook_context_estimate(notebook: string, session: string | null): Promise<ContextEstimate>
  insight_run(source: string, instruction: InsightInstruction): Promise<InsightRun>
  insight_promote(source: string, block: string, notebook: string | null): Promise<string>
  insight_skills(): Promise<string[]>
}

const CONTEXT_METHODS = [
  'notebook_context_set',
  'notebook_context_clear',
  'notebook_context_set_all',
  'notebook_context_estimate',
  'insight_run',
  'insight_promote',
  'insight_skills',
] as const satisfies readonly (keyof ContextApi)[]

export const contextApi = (client: RpcClient): ContextApi =>
  Object.fromEntries(CONTEXT_METHODS.map((name) => [name, (...args: unknown[]) => client.call(name, args)])) as unknown as ContextApi

/** The connected engine's wave-2 methods; like `useApi`, only rendered once connected. */
export function useContextApi(): ContextApi {
  const client = useConnection((s) => s.client)
  if (!client) throw new Error('useContextApi before the connection opened')
  return contextApi(client)
}

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
  /** `notebook_context_estimate(notebook, session)` (§5.2); `null` session = the default rule. */
  estimate: (notebook: string, session: string | null) => ['notebooks', 'estimate', notebook, session] as const,
  insightSkills: ['notebooks', 'insight-skills'] as const,
}
