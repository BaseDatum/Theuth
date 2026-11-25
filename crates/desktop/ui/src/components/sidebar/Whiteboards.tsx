import { useMutation } from '@tanstack/react-query'
import { Link, useNavigate, useRouterState } from '@tanstack/react-router'
import { Plus, Presentation } from 'lucide-react'
import { type ReactNode, useState } from 'react'
import { ago } from '@/components/sidebar/AgentTree'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { SidebarMenuButton, SidebarMenuItem } from '@/components/ui/sidebar'
import { useCreateWhiteboard, useWhiteboards } from '@/components/whiteboard/useWhiteboards'
import { notify } from '@/lib/notify'
import { cn } from '@/lib/utils'
import type { DocSummary } from '@/rpc/bindings/method.agent.output'
import { POPOVER_CAP } from './Rail'

const ROW = 'flex h-8 w-full min-w-0 items-center gap-2 rounded-md px-2 text-left text-sm outline-hidden hover:bg-accent hover:text-accent-foreground focus-visible:bg-accent pointer-coarse:h-10'

/** The popover's rows: New whiteboard, the newest `POPOVER_CAP` whiteboards, All whiteboards. Pure, for the test. */
export function WhiteboardsMenu({ rows, active, creating, onNew, onPick }: {
  rows: readonly DocSummary[]
  active?: string
  creating?: boolean
  onNew(): void
  onPick(): void
}) {
  return (
    <div className="flex flex-col gap-0.5" data-whiteboards-menu>
      <button type="button" className={ROW} disabled={creating} onClick={onNew}>
        <Plus className="size-4 shrink-0" /> <span className="truncate">New whiteboard</span>
      </button>
      {rows.length > 0 && <div className="my-1 h-px bg-border" />}
      <ul>
        {rows.slice(0, POPOVER_CAP).map((w) => (
          <li key={w.id}>
            <Link to="/whiteboards/$id" params={{ id: w.id }} className={cn(ROW, w.id === active && 'bg-accent')} onClick={onPick} data-whiteboard={w.id}>
              <span className="min-w-0 flex-1 truncate">{w.title || 'Untitled whiteboard'}</span>
              <span className="shrink-0 text-[11px] text-fg-tertiary">{ago(new Date(Number(w.updated_ms)).toISOString())}</span>
            </Link>
          </li>
        ))}
      </ul>
      <div className="my-1 h-px bg-border" />
      <Link to="/whiteboards" className={cn(ROW, 'text-xs text-fg-secondary')} onClick={onPick}>
        All whiteboards{rows.length > POPOVER_CAP ? ` (${rows.length})` : ''} →
      </Link>
    </div>
  )
}

/** The open whiteboard's id (`/whiteboards/<id>`), `''` on the list page, undefined elsewhere. */
function useWhiteboardHere(): string | undefined {
  const path = useRouterState({ select: (s) => s.location.pathname })
  return path === '/whiteboards' ? '' : path.startsWith('/whiteboards/') ? decodeURIComponent(path.slice('/whiteboards/'.length)) : undefined
}

/** One popover listing the whiteboards around `trigger` (the sidebar row or the rail icon). */
function WhiteboardsPopover({ trigger, side, active, wrap = (t) => t }: { trigger: ReactNode; side: 'right' | 'bottom'; active: string | undefined; wrap?(trigger: ReactNode): ReactNode }) {
  const [open, setOpen] = useState(false)
  const navigate = useNavigate()
  const list = useWhiteboards()
  const createWhiteboard = useCreateWhiteboard()
  const create = useMutation({
    mutationFn: () => createWhiteboard(),
    onSuccess: (id) => {
      setOpen(false)
      void navigate({ to: '/whiteboards/$id', params: { id } })
    },
    onError: (e) => notify.error(e.message),
  })
  return (
    <Popover open={open} onOpenChange={setOpen}>
      {wrap(<PopoverTrigger asChild>{trigger}</PopoverTrigger>)}
      <PopoverContent side={side} align="start" className="w-64 p-1">
        <WhiteboardsMenu rows={list.data ?? []} active={active} creating={create.isPending} onNew={() => create.mutate()} onPick={() => setOpen(false)} />
      </PopoverContent>
    </Popover>
  )
}

/** 218: the expanded sidebar's Whiteboards row (near Note); its popover lists the newest whiteboards. */
export function WhiteboardsRow({ className }: { className?: string }) {
  const active = useWhiteboardHere()
  return (
    <SidebarMenuItem>
      <WhiteboardsPopover
        side="right"
        active={active}
        trigger={
          <SidebarMenuButton isActive={active !== undefined} className={className} aria-label="Whiteboards">
            <Presentation /> <span>Whiteboards</span>
          </SidebarMenuButton>
        }
      />
    </SidebarMenuItem>
  )
}

/** 218: the collapsed rail's Whiteboards icon, same popover. `wrap` puts the rail tooltip around the trigger. */
export function WhiteboardsRail({ itemClass, wrap }: { itemClass: string; wrap(trigger: ReactNode): ReactNode }) {
  const active = useWhiteboardHere()
  return (
    <WhiteboardsPopover
      side="right"
      active={active}
      wrap={wrap}
      trigger={
        <button type="button" aria-label="Whiteboards" className={itemClass} data-active={active !== undefined || undefined}>
          <Presentation className="size-[18px]" />
        </button>
      }
    />
  )
}
