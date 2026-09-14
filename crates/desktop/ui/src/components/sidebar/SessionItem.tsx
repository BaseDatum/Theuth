import { useQueryClient } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { MoreHorizontal } from 'lucide-react'
import { memo, useState } from 'react'
import { toast } from 'sonner'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Input } from '@/components/ui/input'
import { SidebarMenuAction, SidebarMenuButton } from '@/components/ui/sidebar'
import { queryKeys, useApi } from '@/lib/rpc'
import { useSessionField } from '@/store/sessions'

/** One chat in the list: title (live from `TitleChanged`), active highlight, hover `…` menu with inline rename. */
export const SessionItem = memo(function SessionItem({ id, title, active }: { id: string; title: string; active: boolean }) {
  const api = useApi()
  const qc = useQueryClient()
  const live = useSessionField(id, 'title')
  const shown = live || title || id.slice(0, 8)
  const [editing, setEditing] = useState<string | null>(null)

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
          <SidebarMenuButton asChild isActive={active} className="h-9 rounded-[10px] px-2.5">
            <Link to="/c/$session" params={{ session: id }} title={shown}>
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
              <DropdownMenuItem onSelect={() => void navigator.clipboard.writeText(id)}>Copy id</DropdownMenuItem>
              <DropdownMenuItem disabled>
                Delete
                <span className="ml-auto text-xs text-fg-tertiary">Not available yet</span>
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </>
      )}
    </div>
  )
})
