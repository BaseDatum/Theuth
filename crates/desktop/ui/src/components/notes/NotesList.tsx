import { useMutation, useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import { MoreHorizontal, PanelLeft, Pencil, Plus, StickyNote, Trash2 } from 'lucide-react'
import { type KeyboardEvent, useState } from 'react'
import { toast } from 'sonner'
import { MeetingPill } from '@/components/meeting/MeetingPill'
import { useNotes } from '@/components/notes/NotePicker'
import { ago } from '@/components/sidebar/AgentTree'
import { SearchTrigger } from '@/components/sidebar/SearchTrigger'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { useSidebar } from '@/components/ui/sidebar'
import { Skeleton } from '@/components/ui/skeleton'
import { isMacDesktop } from '@/lib/hotkeys'
import { cn } from '@/lib/utils'
import { queryKeys, useApi } from '@/lib/rpc'
import type { DocSummary } from '@/rpc/bindings/DocSummary'

/** `/notes` (59 §7.3): every note, newest update first; New note; per-row Rename (inline) and Delete (one click). */
export function NotesList() {
  const api = useApi()
  const navigate = useNavigate()
  const notes = useNotes()
  const sidebar = useSidebar()
  const mac = isMacDesktop()
  // The note opens with its title focused (`?new`); a failed create (e.g. the documents helper missing) toasts and stays.
  const create = useMutation({
    mutationFn: () => api.doc_create('Untitled', null),
    onSuccess: (n) => void navigate({ to: '/notes/$id', params: { id: n.id }, search: { new: true } }),
    onError: (e) => toast.error(e.message),
  })
  return (
    <article className="flex min-h-0 min-w-0 flex-1 flex-col">
      <header
        data-tauri-drag-region={mac || undefined}
        className={cn('flex h-[52px] shrink-0 items-center gap-1 border-b border-border-subtle px-3', !sidebar.open && mac && 'pl-20')}
      >
        {(!sidebar.open || sidebar.isMobile) && (
          <div className="flex shrink-0 items-center">
            <SearchTrigger />
            <Button variant="ghost" size="icon-sm" aria-label="Show sidebar" onClick={sidebar.toggleSidebar}>
              <PanelLeft />
            </Button>
          </div>
        )}
        <h2 className="min-w-0 flex-1 truncate text-sm font-semibold">Notes</h2>
        <Button variant="ghost" size="sm" disabled={create.isPending} onClick={() => create.mutate()}>
          <Plus /> New note
        </Button>
        <MeetingPill />
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto w-full max-w-[768px] p-3">
          {notes.isPending && (
            <div className="space-y-2 p-2">
              <Skeleton className="h-8 w-2/3" />
              <Skeleton className="h-8 w-1/2" />
            </div>
          )}
          {notes.isError && <p className="p-2 text-sm text-danger">{notes.error.message}</p>}
          {notes.data?.length === 0 && (
            <p className="p-6 text-center text-sm text-fg-tertiary">
              No notes yet. New note starts one; the agent's <code>doc_create</code> lands here too.
            </p>
          )}
          <ul>{notes.data?.map((n) => <Row key={n.id} note={n} />)}</ul>
        </div>
      </div>
    </article>
  )
}

function Row({ note }: { note: DocSummary }) {
  const api = useApi()
  const qc = useQueryClient()
  const [renaming, setRenaming] = useState<string | null>(null)
  const rename = useMutation({
    mutationFn: (t: string) => api.doc_rename(note.id, t),
    onSuccess: () => void qc.invalidateQueries({ queryKey: queryKeys.notes }),
    onError: (e) => toast.error(e.message),
  })
  const del = useMutation({
    mutationFn: () => api.doc_delete(note.id),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: queryKeys.notes })
      toast(`Deleted ${note.title || 'the note'}`)
    },
    onError: (e) => toast.error(e.message),
  })
  const commit = () => {
    const t = renaming?.trim()
    setRenaming(null)
    if (t && t !== note.title) rename.mutate(t)
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
      <StickyNote className="size-4 shrink-0 text-fg-tertiary" />
      {renaming !== null ? (
        <input
          autoFocus
          aria-label="Title"
          className="min-w-0 flex-1 bg-transparent py-2 text-sm outline-none"
          value={renaming}
          onChange={(e) => setRenaming(e.target.value)}
          onBlur={commit}
          onKeyDown={onKeyDown}
        />
      ) : (
        <Link to="/notes/$id" params={{ id: note.id }} className="flex min-w-0 flex-1 items-baseline gap-2 py-2 text-sm">
          <span className="min-w-0 flex-1 truncate">{note.title || 'Untitled'}</span>
          {note.kind !== 'note' && <span className="shrink-0 rounded-md bg-layer-2 px-1.5 text-[11px] text-fg-secondary">{note.kind}</span>}
          <span className="shrink-0 text-xs text-fg-tertiary">{ago(new Date(Number(note.updated_ms)).toISOString())}</span>
        </Link>
      )}
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="ghost" size="icon-sm" aria-label={`Actions for ${note.title || 'Untitled'}`} className="opacity-0 group-hover:opacity-100 focus:opacity-100 data-[state=open]:opacity-100">
            <MoreHorizontal />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <DropdownMenuItem onSelect={() => setRenaming(note.title)}>
            <Pencil /> Rename
          </DropdownMenuItem>
          <DropdownMenuItem variant="destructive" disabled={del.isPending} onSelect={() => del.mutate()}>
            <Trash2 /> Delete now
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </li>
  )
}
