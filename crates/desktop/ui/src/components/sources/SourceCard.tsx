// 144 §12.1 (mock `.src`): one source in a notebook's Sources column, and the guest's read-only copy.
// 144 wave 2 §5.1: the 4-segment dial is live — `off · mention · summary · full` per item per thread
// (`notebook_context_set`), a dot on the pinned segment, `×` / right-click → `notebook_context_clear` (back to the
// default rule); without a session the dial writes the device default only (`lib/notebook-context.ts`).
import { useQueryClient } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { X } from 'lucide-react'
import type { ReactNode } from 'react'
import { toast } from 'sonner'
import { dragSource } from '@/components/chat/AttachChip'
import { useEnsureNotebookSession, useNotebookSession } from '@/components/notebooks/NotebookContext'
import { useIsMobile } from '@/hooks/use-mobile'
import { type EstimateItem, useContextDefaults } from '@/lib/notebook-context'
import { notebookKeys, useContextApi, useNotebooksApi } from '@/lib/notebooks-rpc'
import type { ContextLevel } from "@/rpc/bindings/method.agent.output"
import type { ItemKind } from "@/rpc/bindings/method.agent.output"
import type { Source as NotebookSource } from "@/rpc/bindings/method.document.output"
import { cn } from '@/lib/utils'
import { LEVELS, type StatusTone, originGlyph, originLabel, originLine, sourceTitle, statusWord } from './origin'

const DOT: Record<StatusTone, string> = {
  ok: 'bg-emerald-600',
  run: 'bg-blue-600',
  fail: 'bg-danger',
  muted: 'bg-fg-tertiary',
}

const PILL = 'inline-flex items-center rounded-full border px-1.5 text-[10px] leading-4 whitespace-nowrap'

export function SourceCard({ source, notebook, readOnly, onOpen, context }: { source: NotebookSource; notebook?: string; readOnly?: boolean; onOpen?: () => void; context?: EstimateItem }) {
  const glyph = originGlyph(source.origin)
  const title = sourceTitle(source)
  const status = statusWord(source)
  const id = source.doc.id
  const failed = source.status === 'failed'
  return (
    <article
      id={`item-${id}`}
      className={cn('rounded-lg border border-border-subtle bg-background p-2.5', context?.status === 'omitted' && 'border-dashed')}
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
        {context?.status === 'truncated' && <span className={cn(PILL, 'border-amber-600/50 text-amber-700')}>truncated</span>}
        {context?.status === 'omitted' && <span className={cn(PILL, 'border-border-subtle')}>omitted</span>}
      </div>
      {!readOnly && notebook && <Dial notebook={notebook} item={id} kind="source" context={context} className="mt-1.5" />}
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

/**
 * What the dial writes: the device default always, then the session's row — creating the notebook's thread first when
 * there is none yet (`useEnsureNotebookSession`), so the first message already carries the dial (§5.1 W-g).
 */
export function useDial(notebook: string) {
  const ctx = useContextApi()
  const qc = useQueryClient()
  const session = useNotebookSession(notebook)
  const ensure = useEnsureNotebookSession(notebook)
  const defaults = useContextDefaults()
  const done = () => void qc.invalidateQueries({ queryKey: ['notebooks', 'estimate', notebook] })
  const fail = (e: unknown) => toast.error(e instanceof Error ? e.message : String(e))
  return {
    session,
    set(item: string, kind: ItemKind, level: ContextLevel) {
      defaults.set(notebook, item, kind, level)
      ensure()
        .then((s) => ctx.notebook_context_set(s, item, kind, level))
        .then(done, fail)
    },
    clear(item: string) {
      defaults.clear(notebook, item)
      ensure()
        .then((s) => ctx.notebook_context_clear(s, item))
        .then(done, fail)
    },
    setAll(items: readonly { id: string; kind: ItemKind }[], kind: ItemKind, level: ContextLevel) {
      defaults.setAll(notebook, items, level)
      ensure()
        .then((s) => ctx.notebook_context_set_all(s, notebook, kind, level))
        .then(done, fail)
    },
  }
}

/**
 * The 4-segment dial of one item. Selected = the estimate's effective level (the session's row or the default rule);
 * a dot marks a pinned segment; with no session yet the remembered device default is shown pinned. Phone: `O · M · S · F`.
 */
export function Dial({ notebook, item, kind, context, className }: { notebook: string; item: string; kind: ItemKind; context?: EstimateItem; className?: string }) {
  const dial = useDial(notebook)
  const mobile = useIsMobile()
  const remembered = useContextDefaults((s) => s.defaults[notebook]?.[item])
  const level = dial.session ? context?.level : (remembered?.level ?? context?.level)
  const pinned = dial.session ? context?.pinned === true : remembered !== undefined
  return (
    <div className={cn('flex items-center gap-1', className)}>
      <div
        role="radiogroup"
        aria-label="Context"
        title="How much of this item the model sees in this thread; right-click resets to the default rule"
        className="grid min-w-0 flex-1 grid-cols-4 overflow-hidden rounded-md border border-border-subtle text-[10px]"
        onContextMenu={(e) => {
          e.preventDefault()
          if (pinned) dial.clear(item)
        }}
      >
        {LEVELS.map((l) => {
          const on = l === level
          return (
            <button
              key={l}
              type="button"
              role="radio"
              aria-checked={on}
              data-pinned={(on && pinned) || undefined}
              className={cn('relative truncate border-l border-border-subtle px-0.5 py-0.5 text-fg-tertiary first:border-l-0 hover:bg-layer-2', on && 'bg-primary font-semibold text-primary-foreground hover:bg-primary')}
              onClick={() => dial.set(item, kind, l)}
            >
              {mobile ? l[0]!.toUpperCase() : l}
              {on && pinned && <span aria-hidden className="absolute top-0.5 right-0.5 size-1 rounded-full bg-primary-foreground" />}
            </button>
          )
        })}
      </div>
      {pinned && (
        <button type="button" aria-label="Back to the default rule" title="Back to the default rule" className="shrink-0 rounded text-fg-tertiary hover:text-fg" onClick={() => dial.clear(item)}>
          <X className="size-3" />
        </button>
      )}
    </div>
  )
}
