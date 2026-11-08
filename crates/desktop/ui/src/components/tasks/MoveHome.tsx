import { ChevronRight, MoreHorizontal } from 'lucide-react'
import { useRef } from 'react'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { cn } from '@/lib/utils'
import type { Task } from '@/rpc/bindings/Task'
import { homeTargets } from './home'
import { useEngineAgents, useFixedSessions, useMesh, useMoveHome } from './queries'

/** The Move to ▸ rows of one task (`homeTargets`) and a refresh of what they depend on when the menu opens. */
function useTargets(task: Task) {
  const mesh = useMesh()
  const roster = useEngineAgents()
  const fixed = useFixedSessions()
  return {
    targets: homeTargets(task, mesh.data, roster.data ?? [], fixed),
    refresh: (open: boolean) => {
      if (open) {
        void mesh.refetch()
        void roster.refetch()
      }
    },
  }
}

/** 182: Move to ▸ rows, laid out like the session menu's: dot · name, the subtitle under it (the missing agent follows the name). */
function MoveItems({ task }: { task: Task }) {
  const move = useMoveHome()
  const { targets } = useTargets(task)
  return targets.map((t) => (
    <DropdownMenuItem key={t.id} disabled={!t.enabled || move.isPending} onSelect={() => move.mutate({ id: task.id, to: t.id, name: t.name })} className="flex-col items-start gap-0">
      <span className="flex items-center gap-1.5">
        <span className={cn('size-1.5 rounded-full', t.online ? 'bg-success' : 'bg-fg-tertiary')} />
        {t.name}
        {t.inline && <span className="text-fg-tertiary"> — {t.detail}</span>}
      </span>
      {t.detail && !t.inline && <span className="pl-3 text-[11px] text-fg-tertiary">{t.detail}</span>}
    </DropdownMenuItem>
  ))
}

/** The detail's ⋯ menu: Reassign… (focuses the owner picker), Move to ▸, Copy task id, Cancel task. */
export function TaskMenu({ task, onCancel }: { task: Task; onCancel: (() => void) | null }) {
  const { targets, refresh } = useTargets(task)
  // Radix hands focus back to the trigger when the menu closes; Reassign… must keep it on the owner picker.
  const reassign = useRef(false)
  return (
    <DropdownMenu onOpenChange={refresh}>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="icon-sm" aria-label="Task menu">
          <MoreHorizontal />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="end"
        className="w-60"
        onCloseAutoFocus={(e) => {
          if (!reassign.current) return
          reassign.current = false
          e.preventDefault()
          document.getElementById('task-owner')?.focus()
        }}
      >
        <DropdownMenuItem onSelect={() => (reassign.current = true)}>Reassign…</DropdownMenuItem>
        {targets.length > 0 && (
          <DropdownMenuSub>
            <DropdownMenuSubTrigger>Move to</DropdownMenuSubTrigger>
            <DropdownMenuSubContent className="w-64">
              <MoveItems task={task} />
            </DropdownMenuSubContent>
          </DropdownMenuSub>
        )}
        <DropdownMenuItem onSelect={() => void navigator.clipboard.writeText(task.id)}>Copy task id</DropdownMenuItem>
        {onCancel && (
          <>
            <DropdownMenuSeparator />
            <DropdownMenuItem className="text-danger" onSelect={onCancel}>
              Cancel task
            </DropdownMenuItem>
          </>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

/** The offline-home callout's button: the same list as the ⋯ menu's submenu. */
export function MoveButton({ task }: { task: Task }) {
  const { refresh } = useTargets(task)
  return (
    <DropdownMenu onOpenChange={refresh}>
      <DropdownMenuTrigger asChild>
        <Button size="sm" variant="outline">
          Move to <ChevronRight />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-64">
        <MoveItems task={task} />
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

/** A board card's ⋯: only Move to ▸ (the card has no other actions). Hidden when there is nowhere to move it. */
export function CardMenu({ task }: { task: Task }) {
  const { targets, refresh } = useTargets(task)
  if (targets.length === 0) return null
  return (
    // The menu portals out of the card's link in the DOM but not in React: keep its pointer events off the card (drag, open).
    <span className="absolute top-1.5 right-1.5" onPointerDown={(e) => e.stopPropagation()} onClick={(e) => e.stopPropagation()}>
      <DropdownMenu onOpenChange={refresh}>
        <DropdownMenuTrigger asChild>
          <Button variant="ghost" size="icon-xs" aria-label="Move task menu">
            <MoreHorizontal />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-64">
          <DropdownMenuSub>
            <DropdownMenuSubTrigger>Move to</DropdownMenuSubTrigger>
            <DropdownMenuSubContent className="w-64">
              <MoveItems task={task} />
            </DropdownMenuSubContent>
          </DropdownMenuSub>
        </DropdownMenuContent>
      </DropdownMenu>
    </span>
  )
}
