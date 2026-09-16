import { useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import { Check, MoreHorizontal } from 'lucide-react'
import { memo, useState } from 'react'
import { toast } from 'sonner'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Input } from '@/components/ui/input'
import { SidebarMenuAction, SidebarMenuButton } from '@/components/ui/sidebar'
import { touch } from '@/lib/hotkeys'
import { queryKeys, useApi } from '@/lib/rpc'
import type { SelectMode } from '@/lib/selection'
import { cn } from '@/lib/utils'
import { useSessionField } from '@/store/sessions'

/**
 * One chat in the list: title (live from `TitleChanged`), active highlight, hover `…` menu with
 * inline rename and delete. ⌘-click / ⇧-click / menu > Select feed the list's multi-selection.
 */
export const SessionItem = memo(function SessionItem({
  id,
  title,
  active,
  selected,
  selecting,
  select,
}: {
  id: string
  title: string
  active: boolean
  selected: boolean
  selecting: boolean
  select(id: string, mode: SelectMode): void
}) {
  const api = useApi()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const live = useSessionField(id, 'title')
  const shown = live || title || id.slice(0, 8)
  const [editing, setEditing] = useState<string | null>(null)
  const [confirming, setConfirming] = useState(false)

  const remove = async () => {
    try {
      await api.delete_session(id)
      if (active) void navigate({ to: '/' })
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }

  const rename = async (value: string) => {
    setEditing(null)
    if (!value.trim() || value === shown) return
    try {
      await api.rename_session(id, value.trim())
      void qc.invalidateQueries({ queryKey: queryKeys.sessions })
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }

  return (
    <div className="group/menu-item relative">
      {editing !== null ? (
        <Input
          autoFocus
          value={editing}
          onChange={(e) => setEditing(e.target.value)}
          onBlur={() => setEditing(null)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') void rename(editing)
            if (e.key === 'Escape') setEditing(null)
          }}
          className="h-9 rounded-[10px] text-sm"
        />
      ) : (
        <>
          <SidebarMenuButton
            asChild
            isActive={active}
            aria-selected={selected}
            className={cn('h-9 rounded-[10px] px-2.5', selected && 'bg-sidebar-accent')}
          >
            <Link
              to="/c/$session"
              params={{ session: id }}
              title={shown}
              onClick={(e) => {
                if (!touch() && (selecting || e.metaKey || e.shiftKey)) {
                  e.preventDefault()
                  select(id, e.shiftKey ? 'range' : 'toggle')
                } else {
                  select(id, 'clear')
                }
              }}
            >
              {selecting && (
                <span
                  className={cn(
                    'flex size-4 shrink-0 items-center justify-center rounded-[4px] border border-fg-tertiary',
                    selected && 'border-foreground bg-foreground text-background',
                  )}
                >
                  {selected && <Check className="size-3 stroke-[3]" />}
                </span>
              )}
              <span className="truncate">{shown}</span>
            </Link>
          </SidebarMenuButton>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <SidebarMenuAction showOnHover className="top-2 right-2" aria-label="Chat options">
                <MoreHorizontal />
              </SidebarMenuAction>
            </DropdownMenuTrigger>
            <DropdownMenuContent side="right" align="start" className="w-48">
              <DropdownMenuItem onSelect={() => setEditing(shown)}>Rename</DropdownMenuItem>
              <DropdownMenuItem onSelect={() => select(id, 'toggle')}>
                {selected ? 'Deselect' : 'Select'}
              </DropdownMenuItem>
              <DropdownMenuItem onSelect={() => void navigator.clipboard.writeText(id)}>
                Copy id
              </DropdownMenuItem>
              <DropdownMenuItem variant="destructive" onSelect={() => setConfirming(true)}>
                Delete
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
          <AlertDialog open={confirming} onOpenChange={setConfirming}>
            <AlertDialogContent>
              <AlertDialogHeader>
                <AlertDialogTitle>Delete chat?</AlertDialogTitle>
                <AlertDialogDescription>
                  “{shown}” and any subagent chats under it are removed for good.
                </AlertDialogDescription>
              </AlertDialogHeader>
              <AlertDialogFooter>
                <AlertDialogCancel>Cancel</AlertDialogCancel>
                <AlertDialogAction onClick={() => void remove()}>Delete</AlertDialogAction>
              </AlertDialogFooter>
            </AlertDialogContent>
          </AlertDialog>
        </>
      )}
    </div>
  )
})
