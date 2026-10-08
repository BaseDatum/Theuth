import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import { AppWindow, MoreHorizontal, Plus, X } from 'lucide-react'
import { type DragEvent, useState } from 'react'
import { toast } from 'sonner'
import { citationCount } from '@/components/chat/Citations'
import { firstLine, useNotebookItems } from '@/components/notebooks/useNotebook'
import { createNote } from '@/components/notes/create'
import { useNotes } from '@/components/notes/useNotes'
import { ago } from '@/components/sidebar/AgentTree'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { useIsMobile } from '@/hooks/use-mobile'
import { DRAG_TYPE, droppedRef } from '@/lib/attach'
import { isProvenanceLine } from '@/lib/citations'
import { notebookKeys, useNotebooksApi } from '@/lib/notebooks-rpc'
import { canPopout, noteOpenMode, openPopout } from '@/lib/popout'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { DocSummary } from '@/rpc/bindings/DocSummary'

/**
 * 144 §12.1 Notes: the notebook's notes, newest added first — paper glyph, title, `edited <ago>`, an `AI · from chat ·
 * N citations kept` line for a saved answer (§8), kind badges. **+ New note** creates and adds (a desktop pops it out,
 * 143 D3); a note dragged from Library onto the column is added. Below, the Overviews placeholder (wave 3).
 */
export function NotesColumn({ notebook }: { notebook: string }) {
  const api = useApi()
  const nb = useNotebooksApi()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const mobile = useIsMobile()
  const items = useNotebookItems(notebook)
  const notes = useNotes()
  const byId = new Map((notes.data ?? []).map((n) => [n.id, n]))
  const rows = (items.data ?? [])
    .filter((i) => i.kind === 'note')
    .sort((a, b) => Number(b.added_ms - a.added_ms))
    .map((i) => ({ id: i.item, summary: byId.get(i.item) }))
  const refresh = () => void qc.invalidateQueries({ queryKey: notebookKeys.items(notebook) })
  const add = useMutation({
    mutationFn: (id: string) => nb.notebook_item_add(notebook, 'note', id),
    onSuccess: refresh,
    onError: (e) => toast.error(e.message),
  })
  const create = useMutation({
    mutationFn: async () => {
      const n = await createNote(api, qc, notes.data ?? [])
      await nb.notebook_item_add(notebook, 'note', n.id)
      return n
    },
    onSuccess: (n) => {
      refresh()
      if (noteOpenMode(mobile, canPopout()) === 'popout') void openPopout({ kind: 'note', doc: n.id, fresh: true })
      else void navigate({ to: '/notes/$id', params: { id: n.id }, search: { new: true } })
    },
    onError: (e) => toast.error(e.message),
  })
  const [over, setOver] = useState(false)
  const onDragOver = (e: DragEvent) => {
    if (!e.dataTransfer.types.includes(DRAG_TYPE)) return
    e.preventDefault()
    setOver(true)
  }
  const onDrop = (e: DragEvent) => {
    setOver(false)
    const r = droppedRef(e.dataTransfer)
    if (!r || r.kind !== 'note') return
    e.preventDefault()
    if (!rows.some((row) => row.id === r.id)) add.mutate(r.id)
  }
  return (
    <section
      className={cn('flex min-h-0 min-w-0 flex-1 flex-col', over && 'ring-1 ring-primary/40 ring-inset')}
      aria-label="Notes"
      onDragOver={onDragOver}
      onDragLeave={() => setOver(false)}
      onDrop={onDrop}
    >
      <header className="flex h-10 shrink-0 items-center gap-2 border-b border-border-subtle px-3 text-xs font-semibold">
        <span>
          Notes <span className="font-normal text-fg-tertiary">· {rows.length}</span>
        </span>
        <Button size="sm" variant="ghost" className="ml-auto" disabled={create.isPending} onClick={() => create.mutate()}>
          <Plus /> New note
        </Button>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto p-2">
        {items.isError && <p className="p-2 text-sm text-danger">{items.error.message}</p>}
        {items.isSuccess && rows.length === 0 && <p className="p-4 text-center text-sm text-fg-tertiary">No notes yet {'\u2014'} save an answer from the chat, or New note.</p>}
        <ul className="flex flex-col gap-1.5">
          {rows.map((r) => (
            <Row key={r.id} id={r.id} summary={r.summary} onRemove={() => nb.notebook_item_remove(notebook, 'note', r.id).then(refresh, (e: Error) => toast.error(e.message))} />
          ))}
        </ul>
        <h3 className="mt-3 px-1 text-[11px] uppercase tracking-wider text-fg-tertiary">Overviews</h3>
        <div className="mt-1 rounded-lg border border-dashed border-border-subtle px-2.5 py-2 text-xs text-fg-tertiary" title="wave 3" aria-disabled>
          Briefing · FAQ · Timeline · Audio {'\u2014'} wave 3
        </div>
      </div>
    </section>
  )
}

/** The mock's `.paper`: a sheet with three ruled lines. */
function Paper() {
  return (
    <span aria-hidden className="flex h-[30px] w-[22px] shrink-0 flex-col gap-[3px] rounded-sm border border-border-subtle bg-background px-1 pt-[5px]">
      <span className="h-0.5 bg-border-subtle" />
      <span className="h-0.5 bg-border-subtle" />
      <span className="h-0.5 bg-border-subtle" />
    </span>
  )
}

function Row({ id, summary, onRemove }: { id: string; summary: DocSummary | undefined; onRemove: () => void }) {
  const api = useApi()
  const md = useQuery({ queryKey: ['doc', 'markdown', id], queryFn: () => api.doc_markdown(id) })
  const title = summary?.title || firstLine(md.data) || 'Untitled'
  const saved = !!md.data && isProvenanceLine(md.data)
  const kind = summary?.kind
  return (
    <li className="group flex items-center gap-2.5 rounded-lg border border-border-subtle bg-background px-2.5 py-2">
      <Paper />
      <Link to="/notes/$id" params={{ id }} search={{}} className="flex min-w-0 flex-1 flex-col">
        <span className="truncate text-[12.5px] font-semibold">{title}</span>
        <span className="flex flex-wrap items-center gap-1 text-[11px] text-fg-tertiary">
          {saved ? (
            <>
              <span className="rounded-full border border-emerald-700/50 px-1.5 text-emerald-700">AI</span> from chat · {citationCount(md.data ?? '')} citations kept
            </>
          ) : (
            summary && <span>edited {ago(new Date(Number(summary.updated_ms)).toISOString())}</span>
          )}
          {kind === 'meeting_note' && <span className="rounded-md bg-layer-2 px-1.5 text-fg-secondary">meeting note</span>}
          {kind === 'board' && <span className="rounded-md bg-layer-2 px-1.5 text-fg-secondary">board</span>}
        </span>
      </Link>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="ghost" size="icon-sm" aria-label={`Actions for ${title}`} className="opacity-0 group-hover:opacity-100 focus:opacity-100 data-[state=open]:opacity-100">
            <MoreHorizontal />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          {canPopout() && (
            <DropdownMenuItem onSelect={() => void openPopout({ kind: 'note', doc: id })}>
              <AppWindow /> Open in new window
            </DropdownMenuItem>
          )}
          <DropdownMenuItem variant="destructive" onSelect={onRemove}>
            <X /> Remove from notebook
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </li>
  )
}
