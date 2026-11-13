import type { QueryClient } from '@tanstack/react-query'
import { toast } from 'sonner'
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
 */
export async function createNote(api: Pick<Api, 'doc_create'>, qc: QueryClient, existing: readonly DocSummary[]): Promise<DocSummary> {
  const known = new Set(existing.map((n) => n.id))
  const { promise: created, resolve } = Promise.withResolvers<DocSummary>()
  let seen: DocSummary | null = null
  const off = on('DocumentChanged', (e) => {
    if (seen || e.kind !== 'note' || e.deleted || e.title !== TITLE || known.has(e.doc)) return
    seen = { id: e.doc, kind: 'note', title: e.title, binding: 'note', created_ms: e.updated_ms, updated_ms: e.updated_ms, bytes: 0 }
    resolve(seen)
  })
  try {
    const n = await Promise.race([
      created,
      api.doc_create(TITLE, null).catch((e: Error) => {
        if (!seen) throw e
        toast.error(e.message)
        return seen
      }),
    ])
    qc.setQueryData<DocSummary[]>(queryKeys.notes, (rows = []) => (rows.some((r) => r.id === n.id) ? rows : [n, ...rows]))
    return n
  } finally {
    off()
  }
}
