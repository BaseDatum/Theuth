import type { EngineProvider } from '@/lib/doc/provider'
import type { DocumentChanged } from "@/rpc/bindings/event.document_2echanged.output"

/**
 * The title the open page shows after `e` (59 §7.3): a rename of `provider`'s document — from this page, the list
 * row or another device — reaches the UI as `DocumentChanged` on the bus, not on the document stream. Writes it
 * into `meta` too (what `docRef` reads). `null` when `e` is another document's or a trash move.
 */
export function titleAfter(provider: Pick<EngineProvider, 'id' | 'meta'>, e: DocumentChanged): string | null {
  if (e.doc !== provider.id || e.deleted) return null
  if (provider.meta) provider.meta.title = e.title
  return e.title
}
