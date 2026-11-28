import { Link, useNavigate } from '@tanstack/react-router'
import { Layers, Plus, Sparkles } from 'lucide-react'
import { SectionBand } from '@/components/nav/SectionBand'
import { SectionRow } from '@/components/nav/SectionRow'
import { ago } from '@/components/sidebar/AgentTree'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { cn } from '@/lib/utils'
import { useCreateDeck, useDecksList } from './useDeckLive'

export const DECKS_BLURB =
  'Collaborative live dashboards with independent sections, review-gated Monty streams, and interactive controls.'

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
          <ul className="space-y-1">
            {list.data?.map((d) => (
              <li key={d.id} className="rounded-[10px] px-2.5 hover:bg-layer-2">
                <Link
                  to="/decks/$id"
                  params={{ id: d.id }}
                  className="flex min-w-0 items-center gap-2.5 py-2.5 text-sm"
                  data-deck-row={d.id}
                >
                  <Layers className="size-4 shrink-0 text-fg-tertiary" />
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2">
                      <span className="truncate font-medium">{d.title || 'Untitled Deck'}</span>
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
                      {d.stale && <span className="text-[10px] text-warning">stale</span>}
                    </div>
                    {d.description && (
                      <p className="truncate text-xs text-fg-tertiary">{d.description}</p>
                    )}
                  </div>
                  <span className="shrink-0 text-xs text-fg-tertiary">
                    {d.section_count} {d.section_count === 1 ? 'section' : 'sections'}
                  </span>
                  <span className="shrink-0 text-xs text-fg-tertiary">
                    {ago(new Date(Number(d.updated_ms)).toISOString())}
                  </span>
                </Link>
              </li>
            ))}
          </ul>
        </div>
      </div>
    </article>
  )
}
