import { Link, useNavigate, useRouterState } from '@tanstack/react-router'
import { Layers, Plus, Sparkles } from 'lucide-react'
import { type ReactNode, useEffect, useRef, useState } from 'react'
import { useViewDrag } from '@/components/chat/Panes'
import { useCreateDeck, useDecksList } from '@/components/decks/useDeckLive'
import type { DeckSummary } from '@/components/decks/types'
import { ago } from '@/components/sidebar/AgentTree'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { SidebarMenuButton, SidebarMenuItem } from '@/components/ui/sidebar'
import { usePanes } from '@/lib/panes'
import { cn } from '@/lib/utils'
import { POPOVER_CAP } from './Rail'

const ROW =
  'flex h-8 w-full min-w-0 items-center gap-2 rounded-md px-2 text-left text-sm outline-hidden hover:bg-accent hover:text-accent-foreground focus-visible:bg-accent pointer-coarse:h-10'

function DeckMenuItem({ d, active, onPick }: { d: DeckSummary; active?: string; onPick(): void }) {
  const title = d.title || 'Untitled Deck'
  const drag = useViewDrag(`/decks/${encodeURIComponent(d.id)}`, title)
  return (
    <li>
      <Link
        to="/decks/$id"
        params={{ id: d.id }}
        {...drag}
        className={cn(ROW, d.id === active && 'bg-accent')}
        onClick={onPick}
        data-deck={d.id}
      >
        <span className="min-w-0 flex-1 truncate">{title}</span>
        {d.state === 'streaming' && (
          <span className="size-1.5 shrink-0 rounded-full bg-success" aria-label="streaming" />
        )}
        <span className="shrink-0 text-[11px] text-fg-tertiary">
          {ago(new Date(Number(d.updated_ms)).toISOString())}
        </span>
      </Link>
    </li>
  )
}

/**
 * The Decks popover menu: New deck, Workspace Pulse (Example), the newest `POPOVER_CAP` decks, and All decks.
 * Pure, directly unit-tested.
 */
export function DecksMenu({
  rows,
  active,
  creating,
  onNew,
  onNewExample,
  onPick,
}: {
  rows: readonly DeckSummary[]
  active?: string
  creating?: boolean
  onNew(): void
  onNewExample?(): void
  onPick(): void
}) {
  const dragAll = useViewDrag('/decks', 'Decks')
  return (
    <div className="flex flex-col gap-0.5" data-decks-menu>
      <button type="button" className={ROW} disabled={creating} onClick={onNew}>
        <Plus className="size-4 shrink-0" /> <span className="truncate">New deck</span>
      </button>
      {onNewExample && (
        <button type="button" className={ROW} disabled={creating} onClick={onNewExample}>
          <Sparkles className="size-4 shrink-0" /> <span className="truncate">Workspace Pulse (Example)</span>
        </button>
      )}
      {rows.length > 0 && <div className="my-1 h-px bg-border" />}
      <ul>
        {rows.slice(0, POPOVER_CAP).map((d) => (
          <DeckMenuItem key={d.id} d={d} active={active} onPick={onPick} />
        ))}
      </ul>
      <div className="my-1 h-px bg-border" />
      <Link to="/decks" {...dragAll} className={cn(ROW, 'text-xs text-fg-secondary')} onClick={onPick}>
        All decks{rows.length > POPOVER_CAP ? ` (${rows.length})` : ''} →
      </Link>
    </div>
  )
}

/** Extract the open deck's id from a pathname (`/decks/<id>`), `''` on `/decks`, undefined elsewhere. */
export function deckIdFromPath(path: string): string | undefined {
  if (path === '/decks') return ''
  if (!path.startsWith('/decks/')) return undefined
  const raw = path.slice('/decks/'.length)
  try {
    return decodeURIComponent(raw)
  } catch {
    return raw
  }
}

/** The open deck's id (`/decks/<id>`), `''` on the list page, undefined elsewhere. */
function useDeckHere(): string | undefined {
  const path = useRouterState({ select: (s) => s.location.pathname })
  return deckIdFromPath(path)
}

/** One popover listing recent decks around `trigger` (expanded row or collapsed rail button). */
function DecksPopover({
  trigger,
  side,
  active,
  wrap = (t) => t,
}: {
  trigger: ReactNode
  side: 'right' | 'bottom'
  active: string | undefined
  wrap?(trigger: ReactNode): ReactNode
}) {
  const [open, setOpen] = useState(false)
  const dragging = usePanes((s) => s.drag !== null)
  const dragged = useRef(false)
  useEffect(() => {
    if (dragging) dragged.current = open
    else if (dragged.current) {
      dragged.current = false
      setOpen(false)
    }
  }, [dragging, open])
  const navigate = useNavigate()
  const list = useDecksList()
  const create = useCreateDeck()

  const createAndOpen = (example: boolean) => {
    create.mutate(example ? { example: true } : { title: 'Untitled Deck' }, {
      onSuccess: (detail) => {
        setOpen(false)
        void navigate({ to: '/decks/$id', params: { id: detail.doc.doc_id } })
      },
    })
  }

  return (
    <Popover open={open} onOpenChange={setOpen}>
      {wrap(<PopoverTrigger asChild>{trigger}</PopoverTrigger>)}
      <PopoverContent side={side} align="start" className={cn('w-64 p-1', dragging && 'pointer-events-none invisible')}>
        <DecksMenu
          rows={list.data ?? []}
          active={active}
          creating={create.isPending}
          onNew={() => createAndOpen(false)}
          onNewExample={() => createAndOpen(true)}
          onPick={() => setOpen(false)}
        />
      </PopoverContent>
    </Popover>
  )
}

/** Expanded sidebar Decks row (beside Whiteboards); its popover lists recent decks + New deck + All decks. */
export function DecksRow({ className }: { className?: string }) {
  const active = useDeckHere()
  return (
    <SidebarMenuItem>
      <DecksPopover
        side="right"
        active={active}
        trigger={
          <SidebarMenuButton isActive={active !== undefined} className={className} aria-label="Decks">
            <Layers /> <span>Decks</span>
          </SidebarMenuButton>
        }
      />
    </SidebarMenuItem>
  )
}

/**
 * Collapsed left rail Decks button: ALWAYS exactly ONE Decks button in the rail regardless of how
 * many Decks exist. Clicking opens the Decks popover (recent decks + New deck + All decks).
 */
export function DecksRail({ itemClass, wrap }: { itemClass: string; wrap(trigger: ReactNode): ReactNode }) {
  const active = useDeckHere()
  return (
    <DecksPopover
      side="right"
      active={active}
      wrap={wrap}
      trigger={
        <button
          type="button"
          aria-label="Decks"
          data-decks-rail-button
          className={itemClass}
          data-active={active !== undefined || undefined}
        >
          <Layers className="size-[18px]" />
        </button>
      }
    />
  )
}
