import type { QueryClient } from '@tanstack/react-query'
import { notify as toast } from '@/lib/notify'
import { queryKeys } from '@/lib/rpc'
import type { Api } from '@/rpc/bindings/client'
import type { DocSummary } from "@/rpc/bindings/method.agent.output"
import { on } from '@/store/sessions'

const TITLE = 'Untitled'

/**
 * New note (59 §7.3, §24.8). `doc_create` writes the meta and index row, publishes `DocumentChanged`, then imports
 * the `# Untitled` body through the documents helper and replies only after that — so a helper that is missing,
 * cold or wedged (30 s deadline) leaves a note that exists and is listed but never opens. The note opens on
 * whichever names its id first: the bus event (a fresh `Untitled` note, none of `existing`) or the reply. The row
 * goes into the list cache at once; a reply that fails after the event still opens the note and toasts the import.
 * `project` files it under that project (the same `doc_create` call; the cached row carries it at once). The engine
 * files it after the body import, so a reply that fails after the event files it here instead.
 */
export async function createNote(
  api: Pick<Api, 'doc_create' | 'doc_set_project'>,
  qc: QueryClient,
  existing: readonly DocSummary[],
  project: string | null = null,
): Promise<DocSummary> {
  const known = new Set(existing.map((n) => n.id))
  const { promise: created, resolve } = Promise.withResolvers<DocSummary>()
  let seen: DocSummary | null = null
  const off = on("document.changed@1", 'DocumentChanged', (e) => {
    if (seen || e.kind !== 'note' || e.deleted || e.title !== TITLE || known.has(e.doc)) return
    seen = { id: e.doc, kind: 'note', title: e.title, binding: 'note', created_ms: e.updated_ms, updated_ms: e.updated_ms, bytes: 0, project }
    resolve(seen)
  })
  try {
    const n = await Promise.race([
      created,
      api.doc_create(TITLE, null, project).catch((e: Error) => {
        if (!seen) throw e
        toast.error(e.message)
        if (project !== null) void api.doc_set_project(seen.id, project).catch((e: Error) => toast.error(e.message))
        return seen
      }),
    ])
    qc.setQueryData<DocSummary[]>(queryKeys.notes, (rows = []) => (rows.some((r) => r.id === n.id) ? rows : [n, ...rows]))
    return n
  } finally {
    off()
  }
}
