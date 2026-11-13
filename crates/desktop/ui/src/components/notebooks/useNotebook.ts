import { useQuerySupport } from '@/rpc/capabilities'
import { useQueries, useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect } from 'react'
import type { ItemKind } from "@/rpc/bindings/method.agent.output"
import type { Notebook } from "@/rpc/bindings/method.agent.output"
import type { Source as NotebookSource } from "@/rpc/bindings/method.document.output"
import type { SourceEvent } from "@/rpc/bindings/event.document_2esources.output"
import type { SourceStatus } from "@/rpc/bindings/method.agent.output"
import { notebookKeys, useContextApi, useNotebooksApi } from '@/lib/notebooks-rpc'
import { useApi } from '@/lib/rpc'
import { on } from '@/store/sessions'


/** `notebook_get(id)` — the row with counts (the items come from `useNotebookItems`). `enabled: false` = no id yet. */
export function useNotebook(id: string, enabled = true) {
  const canQuery1 = useQuerySupport(["notebook_get"])
  const nb = useNotebooksApi()
  return useQuery({ queryKey: notebookKeys.one(id), queryFn: async () => (await nb.notebook_get(id)).notebook, enabled: canQuery1 && (enabled) })
}

/** `notebook_items_list(id)` — live rows only (removed_ms null). */
export function useNotebookItems(id: string) {
  const canQuery2 = useQuerySupport(["notebook_items_list"])
  const nb = useNotebooksApi()
  return useQuery({ enabled: canQuery2,
    queryKey: notebookKeys.items(id),
    queryFn: async () => (await nb.notebook_items_list(id)).filter((i) => i.removed_ms === null),
  })
}

/** `source_list(notebook)`; `SourceEvent`s patch status/progress in place (§12.1 "cards live"). */
export function useSources(notebook: string | null) {
  const canQuery3 = useQuerySupport(["source_list"])
  const nb = useNotebooksApi()
  const qc = useQueryClient()
  useEffect(() => {
    const apply = (raw: unknown) => {
      const ev = sourceEventOf(raw)
      if (!ev) return
      qc.setQueriesData<NotebookSource[]>({ queryKey: ['notebooks', 'sources'] }, (rows) => rows?.map((s) => (s.doc.id === ev.source ? applySourceEvent(s, ev) : s)))
      qc.setQueryData<NotebookSource>(notebookKeys.source(ev.source), (s) => (s ? applySourceEvent(s, ev) : s))
      if (ev.type === 'Changed') void qc.invalidateQueries({ queryKey: ['notebooks', 'sources'] })
    }
    const offs = (['Changed', 'Progress'] as const).map((t) => on('document.sources@1', t, apply))
    return () => offs.forEach((off) => off())
  }, [qc])
  return useQuery({ enabled: canQuery3, queryKey: notebookKeys.sources(notebook), queryFn: () => nb.source_list(notebook) })
}

/** `source_get(id)`. */
export function useSource(id: string) {
  const canQuery4 = useQuerySupport(["source_get"])
  const nb = useNotebooksApi()
  return useQuery({ enabled: canQuery4, queryKey: notebookKeys.source(id), queryFn: () => nb.source_get(id) })
}

/** `notebook_chats(id)` — this notebook's sessions, newest first (§8); owner-only. */
export function useNotebookChats(id: string) {
  const canQuery5 = useQuerySupport(["notebook_chats"])
  const nb = useNotebooksApi()
  return useQuery({ enabled: canQuery5, queryKey: notebookKeys.chats(id), queryFn: () => nb.notebook_chats(id) })
}

/** The notebook document's markdown — the description (§3.2). */
export function useDescription(id: string) {
  const canQuery6 = useQuerySupport(["doc_markdown"])
  const api = useApi()
  return useQuery({ enabled: canQuery6, queryKey: notebookKeys.description(id), queryFn: () => api.doc_markdown(id) })
}

/**
 * 144 wave 2 §5.2: `notebook_context_estimate(notebook, session)`; re-fetched on every dial change (the callers
 * invalidate `notebookKeys.estimate`) and on every `SourceEvent::Changed` (an ingest or insight run changes the tokens).
 */
export function useContextEstimate(notebook: string, session: string | null) {
  const canQuery7 = useQuerySupport(["notebook_context_estimate"])
  const ctx = useContextApi()
  const qc = useQueryClient()
  useEffect(() => {
    const apply = (raw: unknown) => {
      if (sourceEventOf(raw)?.type === 'Changed') void qc.invalidateQueries({ queryKey: ['notebooks', 'estimate', notebook] })
    }
    return on('document.sources@1', 'Changed', apply)
  }, [qc, notebook])
  return useQuery({ enabled: canQuery7, queryKey: notebookKeys.estimate(notebook, session), queryFn: () => ctx.notebook_context_estimate(notebook, session) })
}

/**
 * 144 wave 2 §4 (Sky's ruling): the live notebooks an item belongs to, for the way back from a note or source
 * opened without `?nb`. `notebook_list` + one `notebook_items_list` per notebook, as the source page's Details
 * did (ponytail: O(notebooks); an item→notebooks RPC if it grows). `undefined` while any list is still loading.
 */
export function useItemNotebooks(kind: ItemKind, item: string): Notebook[] | undefined {
  const canQuery8 = useQuerySupport(["notebook_list"])
  const canQuery8_2 = useQuerySupport(["notebook_items_list"])
  const nb = useNotebooksApi()
  const notebooks = useQuery({ enabled: canQuery8, queryKey: notebookKeys.list(false), queryFn: () => nb.notebook_list(false) })
  const lists = useQueries({
    queries: (notebooks.data ?? []).map((n) => ({ enabled: canQuery8_2,
      queryKey: notebookKeys.items(n.doc.id),
      queryFn: async () => (await nb.notebook_items_list(n.doc.id)).filter((i) => i.removed_ms === null),
    })),
  })
  if (!notebooks.data || lists.some((l) => l.isPending)) return undefined
  return notebooks.data.filter((_, i) => lists[i]?.data?.some((it) => it.kind === kind && it.item === item))
}

/** First non-empty line of a markdown body that is not a heading (the notebook document leads with `# <title>`), for the header and the Library row. */
export function firstLine(markdown: string | undefined): string {
  for (const raw of (markdown ?? '').split('\n')) {
    if (/^#+\s/.test(raw)) continue
    const l = raw.replace(/^[*_>`]+|[*_`]+$/g, '').trim()
    if (l) return l
  }
  return ''
}

/** Pure: a bus frame → `SourceEvent`, or null when it is some other event. Tolerates the tag's case. */
export function sourceEventOf(raw: unknown): SourceEvent | null {
  if (!raw || typeof raw !== 'object') return null
  const e = raw as { type?: string; source?: unknown; status?: unknown; percent?: unknown }
  if (typeof e.source !== 'string') return null
  const t = (e.type ?? '').toLowerCase()
  if (t === 'changed' && typeof e.status === 'string') return { type: 'Changed', source: e.source, status: e.status as SourceStatus }
  if (t === 'progress' && typeof e.percent === 'number') return { type: 'Progress', source: e.source, percent: e.percent }
  return null
}

/** Pure: fold one event into a card. A `Changed` sets the status word; a `Progress` only the percent. */
export function applySourceEvent(s: NotebookSource, ev: SourceEvent): NotebookSource {
  return ev.type === 'Changed'
    ? { ...s, status: ev.status, progress: ev.status === 'completed' || ev.status === 'failed' ? null : s.progress }
    : { ...s, status: 'running', progress: ev.percent }
}
