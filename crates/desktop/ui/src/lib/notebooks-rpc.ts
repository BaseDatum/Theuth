// 144 §13.3: the notebook RPCs, typed, over the raw client. `rpc/api.ts`'s `Api` is built from `METHODS`, which must
// equal the server's `bindings/methods.json` (api.test.ts) — only the contract worker extends it. Until then every
// notebook call goes through here; the method names are the frozen §13.3 names, params positional in the order the
// design writes them. Hand-off: the `Api` hunk is in `144-as-built-W4.md`.
import type { DocId } from '@/rpc/bindings/DocId'
import type { DocSummary } from '@/rpc/bindings/DocSummary'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import type { RpcClient } from '@/rpc/client'
import { useConnection } from '@/store/connection'
import type { Citation, ContextEstimate, ContextLevel, ItemKind, Notebook, NotebookItem, Source } from '@/lib/notebooks-types'
import { notebooksFixture, notebooksFixtureOn } from '@/dev/notebooks-fixture'

/** What `source_add` takes besides the notebook: exactly one of these (§13.2). */
export type SourceSpec =
  | { path: string }
  | { url: string }
  | { text: string }
  | { meeting: string }
  | { session: string }
  | { mcp: { server: string; tool: string; args: unknown } }

export type NotebookDeletePreview = { notes: number; sources: number; exclusive_sources: number; chats: number }

export type NotebooksApi = {
  notebook_list(archived: boolean): Promise<Notebook[]>
  notebook_get(id: DocId): Promise<Notebook>
  notebook_create(title: string, description: string | null, project: string | null): Promise<Notebook>
  notebook_update(id: DocId, title: string | null, project: string | null, archived: boolean | null): Promise<Notebook>
  notebook_delete_preview(id: DocId): Promise<NotebookDeletePreview>
  notebook_delete(id: DocId, delete_exclusive_sources: boolean): Promise<null>
  notebook_items_list(notebook: DocId): Promise<NotebookItem[]>
  notebook_item_add(notebook: DocId, kind: ItemKind, item: string): Promise<NotebookItem>
  notebook_item_remove(notebook: DocId, kind: ItemKind, item: string): Promise<null>
  notebook_chats(notebook: DocId): Promise<SessionSummary[]>
  session_set_notebook(session: string, notebook: DocId | null): Promise<null>
  source_add(notebook: DocId, spec: SourceSpec, title: string | null): Promise<Source>
  source_get(id: DocId): Promise<Source>
  source_list(notebook: DocId | null): Promise<Source[]>
  source_refresh(id: DocId): Promise<Source>
  source_insights_run(id: DocId, skill: string | null): Promise<string[]>
  insight_promote(source: DocId, block: string, notebook: DocId | null): Promise<DocSummary>
  notebook_context_set(session: string, items: { kind: ItemKind; item: string; level: ContextLevel }[]): Promise<null>
  notebook_context_estimate(notebook: DocId, session: string | null): Promise<ContextEstimate>
  cite_resolve(hrefs: string[]): Promise<Citation[]>
  save_answer_as_note(session: string, turn: number, notebook: DocId): Promise<DocSummary>
  promote_answer(session: string, turn: number, note: DocId): Promise<null>
}

const METHODS: readonly (keyof NotebooksApi)[] = [
  'notebook_list', 'notebook_get', 'notebook_create', 'notebook_update', 'notebook_delete_preview', 'notebook_delete',
  'notebook_items_list', 'notebook_item_add', 'notebook_item_remove', 'notebook_chats', 'session_set_notebook',
  'source_add', 'source_get', 'source_list', 'source_refresh', 'source_insights_run', 'insight_promote',
  'notebook_context_set', 'notebook_context_estimate', 'cite_resolve', 'save_answer_as_note', 'promote_answer',
]

/** The notebook RPCs over one client (the app's, or a guest's `/d` client). */
export function notebooksApi(client: Pick<RpcClient, 'call'>): NotebooksApi {
  const api: Partial<Record<keyof NotebooksApi, (...args: unknown[]) => Promise<unknown>>> = {}
  for (const name of METHODS) api[name] = (...args) => client.call(name, args)
  return api as NotebooksApi
}

const wrapped = new WeakMap<RpcClient, NotebooksApi>()

/** The connected engine's notebook RPCs; like `useApi`, only rendered once connected. In dev the 144 fixture may answer instead. */
export function useNotebooksApi(): NotebooksApi {
  const client = useConnection((s) => s.client)
  const api = useConnection((s) => s.api)
  if (!client || !api) throw new Error('useNotebooksApi before the connection opened')
  let hit = wrapped.get(client)
  if (!hit) wrapped.set(client, (hit = notebooksFixtureOn() ? notebooksFixture(api) : notebooksApi(client)))
  return hit
}

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
