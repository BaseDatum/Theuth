// 144 §12.1 (mock `.src`): one source in a notebook's Sources column, and the guest's read-only copy.
import { useQueryClient } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import type { ReactNode } from 'react'
import { toast } from 'sonner'
import { dragSource } from '@/components/chat/AttachChip'
import { notebookKeys, useNotebooksApi } from '@/lib/notebooks-rpc'
import type { NotebookSource } from '@/rpc/bindings/NotebookSource'
import { cn } from '@/lib/utils'
import { LEVELS, type StatusTone, originGlyph, originLabel, originLine, sourceTitle, statusWord } from './origin'

const DOT: Record<StatusTone, string> = {
  ok: 'bg-emerald-600',
  run: 'bg-blue-600',
  fail: 'bg-danger',
  muted: 'bg-fg-tertiary',
}

const PILL = 'inline-flex items-center rounded-full border px-1.5 text-[10px] leading-4 whitespace-nowrap'

export function SourceCard({ source, notebook, readOnly, onOpen }: { source: NotebookSource; notebook?: string; readOnly?: boolean; onOpen?: () => void }) {
  const glyph = originGlyph(source.origin)
  const title = sourceTitle(source)
  const status = statusWord(source)
  const id = source.doc.id
  const failed = source.status === 'failed'
  return (
    <article
      className="rounded-lg border border-border-subtle bg-background p-2.5"
      {...dragSource(() => ({ kind: 'source', id, title, meta: originLabel(source.origin) }))}
    >
      <div className="flex items-center gap-2 text-xs font-semibold">
        <span className={cn('flex size-[18px] shrink-0 items-center justify-center rounded text-[10px] font-bold text-white', glyph.className)}>{glyph.text}</span>
        {onOpen ? (
          <button type="button" className="min-w-0 flex-1 truncate text-left hover:underline" onClick={onOpen}>
            {title}
          </button>
        ) : notebook ? (
          <Link to="/sources/$id" params={{ id }} search={{ nb: notebook }} className="min-w-0 flex-1 truncate hover:underline">
            {title}
          </Link>
        ) : (
          <span className="min-w-0 flex-1 truncate">{title}</span>
        )}
        <span className="flex shrink-0 items-center gap-1 text-[10.5px] font-normal">
          <span className={cn('size-1.5 rounded-full', DOT[status.tone])} />
          {failed && !readOnly ? <Refetch id={id}>{status.text}</Refetch> : failed ? 'failed' : status.text}
        </span>
      </div>
      <div className="mt-0.5 flex flex-wrap items-center gap-1.5 text-[11px] text-fg-tertiary">
        {originLine(source)}
        {source.changed_since !== null &&
          (readOnly ? (
            <span className={cn(PILL, 'border-orange-700/50 text-orange-700')}>changed since</span>
          ) : (
            <Refetch id={id} className={cn(PILL, 'border-orange-700/50 text-orange-700 hover:bg-layer-2')}>
              changed since · re-fetch
            </Refetch>
          ))}
        {source.error && <span className={cn(PILL, 'max-w-full truncate border-danger/50 text-danger')}>{source.error}</span>}
      </div>
      {!readOnly && (
        <div role="group" aria-disabled="true" title="wave 2" className="mt-1.5 grid grid-cols-4 overflow-hidden rounded-md border border-border-subtle text-[10px]">
          {LEVELS.map((l) => (
            <button
              key={l}
              type="button"
              disabled
              className={cn('truncate border-l border-border-subtle px-0.5 py-0.5 text-fg-tertiary first:border-l-0', l === 'full' && 'bg-primary font-semibold text-primary-foreground')}
            >
              {l}
            </button>
          ))}
        </div>
      )}
    </article>
  )
}

/** `source_refresh(id)` behind a word; its own component so a read-only card never asks for the owner's RPC. */
function Refetch({ id, className, children }: { id: string; className?: string; children: ReactNode }) {
  const nb = useNotebooksApi()
  const qc = useQueryClient()
  const run = () =>
    nb.source_refresh(id).then(
      () => {
        toast('Re-fetching…')
        void qc.invalidateQueries({ queryKey: notebookKeys.source(id) })
        void qc.invalidateQueries({ queryKey: ['notebooks', 'sources'] })
      },
      (e: unknown) => toast.error(e instanceof Error ? e.message : String(e)),
    )
  return (
    <button type="button" className={className ?? 'hover:underline'} onClick={() => void run()}>
      {children}
    </button>
  )
}
