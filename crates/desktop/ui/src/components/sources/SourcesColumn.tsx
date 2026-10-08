// 144 §12.1: the notebook page's left column — `Sources · N`, `+ Add ▾`, the bulk dial row, the cards.
import { useSources } from '@/components/notebooks/useNotebook'
import { Skeleton } from '@/components/ui/skeleton'
import { AddSourceMenu } from './AddSourceMenu'
import { SourceCard } from './SourceCard'
import { LEVELS } from './origin'

export function SourcesColumn({ notebook }: { notebook: string }) {
  const q = useSources(notebook)
  return (
    <section className="flex min-h-0 min-w-0 flex-1 flex-col" aria-label="Sources">
      <header className="flex h-10 shrink-0 items-center gap-2 border-b border-border-subtle px-3 text-xs font-semibold">
        Sources
        {q.data && <span className="font-normal text-fg-tertiary">· {q.data.length}</span>}
        <span className="ml-auto">
          <AddSourceMenu notebook={notebook} />
        </span>
      </header>
      <div className="flex shrink-0 items-center gap-1.5 whitespace-nowrap border-b border-border-subtle px-3 py-1.5 text-[11px] text-fg-tertiary">
        all sources:
        <div role="group" aria-disabled="true" title="wave 2" className="inline-flex overflow-hidden rounded-md border border-border-subtle text-[10px]">
          {LEVELS.map((l) => (
            <button key={l} type="button" disabled className="border-l border-border-subtle px-1 py-0.5 first:border-l-0">
              {l}
            </button>
          ))}
        </div>
        <span className="ml-auto">~— tokens</span>
      </div>
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
          q.data.map((s) => <SourceCard key={s.doc.id} source={s} notebook={notebook} />)
        )}
      </div>
    </section>
  )
}
