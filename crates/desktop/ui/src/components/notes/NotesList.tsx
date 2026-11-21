import { useMutation, useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import { AppWindow, FolderInput, LayoutPanelTop, MoreHorizontal, Pencil, Plus, StickyNote, Trash2 } from 'lucide-react'
import { type KeyboardEvent, useMemo, useRef, useState } from 'react'
import { toast } from 'sonner'
import { dragSource } from '@/components/chat/AttachChip'
import { ProjectPicker } from '@/components/chat/ProjectPicker'
import { SectionBand } from '@/components/nav/SectionBand'
import { SectionRow } from '@/components/nav/SectionRow'
import { createNote } from '@/components/notes/create'
import { groupNotes, newNoteProject } from '@/components/notes/groups'
import { useNotes } from '@/components/notes/useNotes'
import { ago } from '@/components/sidebar/AgentTree'
import { ProjectHeader } from '@/components/sidebar/SessionList'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { DictationButton } from '@/components/ui/dictation'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Skeleton } from '@/components/ui/skeleton'
import { toRef } from '@/lib/attach'
import { canPopout, openPopout } from '@/lib/popout'
import { queryKeys, useApi } from '@/lib/rpc'
import type { DocSummary } from "@/rpc/bindings/method.agent.output"

/** Header and menu label of a project's new-note action; `null` is the ungrouped bucket. */
const newNoteLabel = (project: string | null) => (project === null ? 'New note (no project)' : `New note in “${project}”`)

/**
 * `/notes` (59 §7.3): every note, newest update first, under collapsible project headers (the chats' projects and
 * styles, `Ungrouped` last; no headers while no note has a project); New note; per-row Rename (inline), Move to
 * project… and Delete (one click). A header's `+` starts a note in its project, and so does New note while that is
 * the only group left open (`newNoteProject`).
 */
export function NotesList() {
  const api = useApi()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const notes = useNotes()
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(() => new Set())
  const rows = useMemo(() => groupNotes(notes.data ?? [], collapsed), [notes.data, collapsed])
  const projects = useMemo(() => [...new Set((notes.data ?? []).flatMap((n) => (n.project ? [n.project] : [])))], [notes.data])
  const here = newNoteProject(rows)
  // The note opens with its title focused (`?new`) as soon as it exists (`create.ts`); a create that fails before that toasts and stays.
  const create = useMutation({
    mutationFn: (project: string | null) => createNote(api, qc, notes.data ?? [], project),
    onSuccess: (n) => void navigate({ to: '/notes/$id', params: { id: n.id }, search: { new: true } }),
    onError: (e) => toast.error(e.message),
  })
  return (
    <article className="flex min-h-0 min-w-0 flex-1 flex-col">
      <SectionBand group="library" />
      <SectionRow>
        <Button variant="ghost" size="sm" className="ml-auto" disabled={create.isPending} onClick={() => create.mutate(here)}>
          <Plus /> {here === null ? 'New note' : newNoteLabel(here)}
        </Button>
      </SectionRow>
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
          <ul>
            {rows.map((r) =>
              'note' in r ? (
                <Row key={r.key} note={r.note} projects={projects} />
              ) : (
                <li key={r.key}>
                  <ProjectHeader
                    project={r.project}
                    collapsed={r.collapsed}
                    toggle={() =>
                      setCollapsed((prev) => {
                        const next = new Set(prev)
                        const k = r.project ?? ''
                        if (!next.delete(k)) next.add(k)
                        return next
                      })
                    }
                    create={{ label: newNoteLabel(r.project), run: () => create.mutate(r.project) }}
                  />
                </li>
              ),
            )}
          </ul>
        </div>
      </div>
    </article>
  )
}

/** Row menu's Move to project…: an existing project (the chats' and the notes'), a new one typed inline, or none. Mounted while open. */
function MoveToProject({ note, projects, onClose }: { note: DocSummary; projects: readonly string[]; onClose(): void }) {
  const api = useApi()
  const qc = useQueryClient()
  const [value, setValue] = useState<string | null>(note.project ?? null)
  const move = useMutation({
    mutationFn: (project: string | null) => api.doc_set_project(note.id, project),
    onSuccess: (row) => {
      qc.setQueryData<DocSummary[]>(queryKeys.notes, (rows = []) => rows.map((r) => (r.id === row.id ? { ...r, project: row.project ?? null } : r)))
      void qc.invalidateQueries({ queryKey: queryKeys.notes })
      onClose()
    },
    onError: (e) => toast.error(e.message),
  })
  const target = value?.trim() || null
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="max-w-sm gap-3 p-4">
        <DialogHeader>
          <DialogTitle className="truncate text-sm">Move “{note.title || 'Untitled'}” to project</DialogTitle>
        </DialogHeader>
        <ProjectPicker value={value} onChange={setValue} extra={projects} />
        <DialogFooter>
          <Button variant="ghost" size="sm" onClick={onClose}>
            Cancel
          </Button>
          <Button size="sm" disabled={move.isPending || target === (note.project ?? null)} onClick={() => move.mutate(target)}>
            {target === null ? 'Remove from project' : 'Move'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function Row({ note, projects }: { note: DocSummary; projects: readonly string[] }) {
  const api = useApi()
  const qc = useQueryClient()
  const [renaming, setRenaming] = useState<string | null>(null)
  const [moving, setMoving] = useState(false)
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
  const renameInput = useRef<HTMLInputElement>(null)
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
      {note.kind === 'board' ? <LayoutPanelTop className="size-4 shrink-0 text-fg-tertiary" aria-label="Board" /> : <StickyNote className="size-4 shrink-0 text-fg-tertiary" />}
      {renaming !== null ? (
        <>
          <input
            ref={renameInput}
            autoFocus
            aria-label="Title"
            className="min-w-0 flex-1 bg-transparent py-2 text-sm outline-none"
            value={renaming}
            onChange={(e) => setRenaming(e.target.value)}
            onBlur={commit}
            onKeyDown={onKeyDown}
          />
          <DictationButton field={renameInput} />
        </>
      ) : (
        <Link
          to="/notes/$id"
          params={{ id: note.id }}
          className="flex min-w-0 flex-1 items-baseline gap-2 py-2 text-sm"
          {...dragSource(() => toRef({ kind: 'note', doc: note }))}
        >
          <span className="min-w-0 flex-1 truncate">{note.title || 'Untitled'}</span>
          {note.kind !== 'note' && <span className="shrink-0 rounded-md bg-layer-2 px-1.5 text-[11px] text-fg-secondary">{note.kind}</span>}
          <span className="shrink-0 text-xs text-fg-tertiary">{ago(new Date(Number(note.updated_ms)).toISOString())}</span>
        </Link>
      )}
      {canPopout() && (
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="Open in new window"
          className="opacity-0 group-hover:opacity-100 focus:opacity-100"
          onClick={() => void openPopout({ kind: 'note', doc: note.id })}
        >
          <AppWindow />
        </Button>
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
          <DropdownMenuItem onSelect={() => setMoving(true)}>
            <FolderInput /> Move to project…
          </DropdownMenuItem>
          {canPopout() && (
            <DropdownMenuItem onSelect={() => void openPopout({ kind: 'note', doc: note.id })}>
              <AppWindow /> Open in new window
            </DropdownMenuItem>
          )}
          <DropdownMenuItem variant="destructive" disabled={del.isPending} onSelect={() => del.mutate()}>
            <Trash2 /> Delete now
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      {moving && <MoveToProject note={note} projects={projects} onClose={() => setMoving(false)} />}
    </li>
  )
}
