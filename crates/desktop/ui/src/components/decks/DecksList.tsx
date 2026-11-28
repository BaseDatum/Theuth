import { useMutation, useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import { Layers, MoreHorizontal, Pencil, Plus, ShieldCheck, Sparkles, Trash2 } from 'lucide-react'
import { type KeyboardEvent, useState } from 'react'
import { useViewDrag } from '@/components/chat/Panes'
import { SectionBand } from '@/components/nav/SectionBand'
import { SectionRow } from '@/components/nav/SectionRow'
import { ago } from '@/components/sidebar/AgentTree'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { Skeleton } from '@/components/ui/skeleton'
import { notify } from '@/lib/notify'
import { cn } from '@/lib/utils'
import { useConnection } from '@/store/connection'
import { type DeckSummary, safeIsoTimestamp } from './types'
import { decksKey, useCreateDeck, useDecksList } from './useDeckLive'

export const DECKS_BLURB =
  'Collaborative live dashboards with independent sections, review-gated Monty streams, and interactive controls.'

function DeckRow({ d }: { d: DeckSummary }) {
  const api = useConnection((s) => s.api)
  const qc = useQueryClient()
  const [renaming, setRenaming] = useState<string | null>(null)
  const title = d.title || 'Untitled Deck'
  const drag = useViewDrag(`/decks/${encodeURIComponent(d.id)}`, title)
  const iso = safeIsoTimestamp(d.updated_ms)

  const rename = useMutation({
    mutationFn: async (t: string) => {
      if (!api) throw new Error('Not connected')
      await api.doc_rename(d.id, t)
    },
    onSuccess: () => void qc.invalidateQueries({ queryKey: decksKey }),
    onError: (e) => notify.error(e instanceof Error ? e.message : String(e)),
  })

  const del = useMutation({
    mutationFn: async () => {
      if (!api) throw new Error('Not connected')
      await api.doc_delete(d.id)
    },
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: decksKey })
      notify(`Deleted ${title}`)
    },
    onError: (e) => notify.error(e instanceof Error ? e.message : String(e)),
  })

  const commit = () => {
    const t = renaming?.trim()
    setRenaming(null)
    if (t && t !== d.title) rename.mutate(t)
  }

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter') e.currentTarget.blur()
    else if (e.key === 'Escape') {
      setRenaming(null)
      e.currentTarget.blur()
    }
  }

  return (
    <li className="group flex items-center gap-2 rounded-[10px] px-2.5 hover:bg-layer-2">
      <Layers className="size-4 shrink-0 text-fg-tertiary" />
      {renaming !== null ? (
        <input
          autoFocus
          aria-label="Deck title"
          className="min-w-0 flex-1 bg-transparent py-2.5 text-sm outline-none"
          value={renaming}
          onChange={(e) => setRenaming(e.target.value)}
          onBlur={commit}
          onKeyDown={onKeyDown}
        />
      ) : (
        <Link
          to="/decks/$id"
          params={{ id: d.id }}
          {...drag}
          className="flex min-w-0 flex-1 items-center gap-2.5 py-2.5 text-sm"
          data-deck-row={d.id}
        >
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2">
              <span className="truncate font-medium">{title}</span>
              <span
                className={cn(
                  'rounded-full px-1.5 py-0.5 text-[10px]',
                  d.state === 'streaming'
                    ? 'bg-success/15 text-success'
                    : d.state === 'waiting_approval' || d.pending_rev
                      ? 'bg-warning/15 text-warning'
                      : 'bg-layer-2 text-fg-tertiary',
                )}
              >
                {d.pending_rev && !d.active_rev ? 'pending review' : d.state}
              </span>
              {d.trusted && (
                <span
                  className="inline-flex items-center gap-0.5 rounded-full bg-fg-accent/15 px-1.5 py-0.5 text-[10px] font-medium text-fg-accent"
                  data-deck-row-trusted
                >
                  <ShieldCheck className="size-3" /> Trusted
                </span>
              )}
              {d.stale && <span className="text-[10px] text-warning">stale</span>}
            </div>
            {d.description && (
              <p className="truncate text-xs text-fg-tertiary">{d.description}</p>
            )}
          </div>
          <span className="shrink-0 text-xs text-fg-tertiary">
            {d.section_count} {d.section_count === 1 ? 'section' : 'sections'}
          </span>
          {iso && (
            <span className="shrink-0 text-xs text-fg-tertiary">
              {ago(iso)}
            </span>
          )}
        </Link>
      )}
      {api && (
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label={`Actions for ${title}`}
              className="opacity-0 group-hover:opacity-100 focus:opacity-100 data-[state=open]:opacity-100"
            >
              <MoreHorizontal />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuItem onSelect={() => setRenaming(d.title)}>
              <Pencil /> Rename
            </DropdownMenuItem>
            <DropdownMenuItem variant="destructive" disabled={del.isPending} onSelect={() => del.mutate()}>
              <Trash2 /> Delete
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      )}
    </li>
  )
}

export function DecksList() {
  const navigate = useNavigate()
  const list = useDecksList()
  const create = useCreateDeck()

  const createAndOpen = (example: boolean) => {
    create.mutate(
      example ? { example: true } : { title: 'Untitled Deck' },
      {
        onSuccess: (detail) => {
          void navigate({ to: '/decks/$id', params: { id: detail.doc.doc_id } })
        },
      },
    )
  }

  return (
    <article className="flex min-h-0 min-w-0 flex-1 flex-col">
      <SectionBand group="library" />
      <SectionRow>
        <h1 className="text-sm font-semibold">Decks</h1>
        <div className="ml-auto flex items-center gap-1.5">
          <Button
            variant="outline"
            size="sm"
            disabled={create.isPending}
            onClick={() => createAndOpen(true)}
          >
            <Sparkles className="size-3.5" /> Workspace Pulse (Example)
          </Button>
          <Button
            variant="ghost"
            size="sm"
            disabled={create.isPending}
            onClick={() => createAndOpen(false)}
          >
            <Plus className="size-3.5" /> New deck
          </Button>
        </div>
      </SectionRow>
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto w-full max-w-[768px] p-3">
          {list.isPending && (
            <div className="space-y-2 p-2">
              <Skeleton className="h-10 w-2/3" />
              <Skeleton className="h-10 w-1/2" />
            </div>
          )}
          {list.isError && <p className="p-2 text-sm text-danger">{list.error.message}</p>}
          {list.data?.length === 0 && (
            <div className="space-y-3 p-6 text-center text-sm text-fg-tertiary">
              <p>No Decks yet. {DECKS_BLURB}</p>
              <div className="flex justify-center gap-2">
                <Button size="sm" onClick={() => createAndOpen(true)} disabled={create.isPending}>
                  <Sparkles className="size-3.5" /> Create Workspace Pulse (Example)
                </Button>
                <Button size="sm" variant="outline" onClick={() => createAndOpen(false)} disabled={create.isPending}>
                  <Plus className="size-3.5" /> New blank deck
                </Button>
              </div>
            </div>
          )}
          {list.data && list.data.length > 0 && (
            <ul className="space-y-1">
              {list.data.map((d) => (
                <DeckRow key={d.id} d={d} />
              ))}
            </ul>
          )}
        </div>
      </div>
    </article>
  )
}
