// 144 §12.1: the notebook page's left column — `Sources · N`, `+ Add ▾`, the bulk dial row, the cards.
// 144 wave 2 §5.1–5.2: the bulk row sets every source (`notebook_context_set_all`, `kind: 'source'`) and shows the
// running `~tokens` of the sources from the estimate; each card gets its estimate item (level, pinned, status).
import { useNotebookSession } from '@/components/notebooks/NotebookContext'
import { useContextEstimate, useSources } from '@/components/notebooks/useNotebook'
import { Skeleton } from '@/components/ui/skeleton'
import { useIsMobile } from '@/hooks/use-mobile'
import { type EstimateItem, fmtTokens } from '@/lib/notebook-context'
import type { ItemKind } from '@/rpc/bindings/ItemKind'
import { AddSourceMenu } from './AddSourceMenu'
import { LEVELS } from './origin'
import { SourceCard, useDial } from './SourceCard'

export function SourcesColumn({ notebook }: { notebook: string }) {
  const q = useSources(notebook)
  const session = useNotebookSession(notebook)
  const estimate = useContextEstimate(notebook, session).data
  const byItem = new Map((estimate?.items ?? []).map((i) => [i.id, i]))
  return (
    <section className="flex min-h-0 min-w-0 flex-1 flex-col" aria-label="Sources">
      <header className="flex h-10 shrink-0 items-center gap-2 border-b border-border-subtle px-3 text-xs font-semibold">
        Sources
        {q.data && <span className="font-normal text-fg-tertiary">· {q.data.length}</span>}
        <span className="ml-auto">
          <AddSourceMenu notebook={notebook} />
        </span>
      </header>
      <BulkRow notebook={notebook} kind="source" label="all sources:" items={(q.data ?? []).map((s) => s.doc.id)} estimate={estimate?.items} />
      <div className="flex flex-col gap-1.5 overflow-y-auto p-2">
        {q.isPending ? (
          <>
            <Skeleton className="h-16" />
            <Skeleton className="h-16" />
          </>
        ) : q.isError ? (
          <p className="px-1 text-xs text-danger">{q.error.message}</p>
        ) : q.data.length === 0 ? (
          <p className="px-1 py-2 text-xs text-fg-tertiary">
            No sources yet. Add a file, a link, a meeting, a chat or paste text — the agent's <code>source_add</code> lands here too.
          </p>
        ) : (
          q.data.map((s) => <SourceCard key={s.doc.id} source={s} notebook={notebook} context={byItem.get(s.doc.id)} />)
        )}
      </div>
    </section>
  )
}

/** `all sources: off · mention · summary · full · ~12.4k tokens` — the Notes column renders the same row for notes. */
export function BulkRow({ notebook, kind, label, items, estimate }: { notebook: string; kind: ItemKind; label: string; items: readonly string[]; estimate: readonly EstimateItem[] | undefined }) {
  const dial = useDial(notebook)
  const mobile = useIsMobile()
  const tokens = estimate?.filter((i) => i.kind === kind && i.status !== 'omitted').reduce((n, i) => n + i.tokens, 0)
  return (
    <div className="flex shrink-0 items-center gap-1.5 whitespace-nowrap border-b border-border-subtle px-3 py-1.5 text-[11px] text-fg-tertiary">
      {label}
      <div role="group" aria-label={label.replace(':', '')} className="inline-flex overflow-hidden rounded-md border border-border-subtle text-[10px]">
        {LEVELS.map((l) => (
          <button
            key={l}
            type="button"
            disabled={items.length === 0}
            className="border-l border-border-subtle px-1 py-0.5 first:border-l-0 hover:bg-layer-2 disabled:opacity-50"
            onClick={() =>
              dial.setAll(
                items.map((id) => ({ id, kind })),
                kind,
                l,
              )
            }
          >
            {mobile ? l[0]!.toUpperCase() : l}
          </button>
        ))}
      </div>
      <span className="ml-auto">{tokens === undefined ? '~— tokens' : `~${fmtTokens(tokens)} tokens`}</span>
    </div>
  )
}
