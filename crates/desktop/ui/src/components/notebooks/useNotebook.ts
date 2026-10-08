import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect } from 'react'
import type { NotebookSource } from '@/rpc/bindings/NotebookSource'
import type { SourceEvent } from '@/rpc/bindings/SourceEvent'
import type { SourceStatus } from '@/rpc/bindings/SourceStatus'
import { notebookKeys, useNotebooksApi } from '@/lib/notebooks-rpc'
import { useApi } from '@/lib/rpc'
import { on } from '@/store/sessions'
import type { WireEvent } from '@/rpc/events'

/** `notebook_get(id)` — the row with counts (the items come from `useNotebookItems`). */
export function useNotebook(id: string) {
  const nb = useNotebooksApi()
  return useQuery({ queryKey: notebookKeys.one(id), queryFn: async () => (await nb.notebook_get(id)).notebook })
}

/** `notebook_items_list(id)` — live rows only (removed_ms null). */
export function useNotebookItems(id: string) {
  const nb = useNotebooksApi()
  return useQuery({
    queryKey: notebookKeys.items(id),
    queryFn: async () => (await nb.notebook_items_list(id)).filter((i) => i.removed_ms === null),
  })
}

/** `source_list(notebook)`; `SourceEvent`s patch status/progress in place (§12.1 "cards live"). */
export function useSources(notebook: string | null) {
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
    // The bus is typed by `WireEvent`, which predates `SourceEvent`; subscribe by the tags §13.1 writes.
    const offs = (['Changed', 'Progress'] as unknown as WireEvent['type'][]).map((t) => on(t, apply))
    return () => offs.forEach((off) => off())
  }, [qc])
  return useQuery({ queryKey: notebookKeys.sources(notebook), queryFn: () => nb.source_list(notebook) })
}

/** `source_get(id)`. */
export function useSource(id: string) {
  const nb = useNotebooksApi()
  return useQuery({ queryKey: notebookKeys.source(id), queryFn: () => nb.source_get(id) })
}

/** `notebook_chats(id)` — this notebook's sessions, newest first (§8); owner-only. */
export function useNotebookChats(id: string) {
  const nb = useNotebooksApi()
  return useQuery({ queryKey: notebookKeys.chats(id), queryFn: () => nb.notebook_chats(id) })
}

/** The notebook document's markdown — the description (§3.2). */
export function useDescription(id: string) {
  const api = useApi()
  return useQuery({ queryKey: notebookKeys.description(id), queryFn: () => api.doc_markdown(id) })
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
