import { useMutation, useQueryClient } from '@tanstack/react-query'
import { Link, getRouteApi, useNavigate } from '@tanstack/react-router'
import { Archive, ArchiveRestore, MoreHorizontal, Notebook as NotebookIcon, Pencil, PictureInPicture2, Plus, Trash2 } from 'lucide-react'
import { type KeyboardEvent, useState } from 'react'
import { toast } from 'sonner'
import { Column, Page } from '@/components/library/Page'
import { AskDialog } from '@/components/notebooks/AskDialog'
import { popOutNotebook } from '@/components/notebooks/NotebookHeaderActions'
import { firstLine, useDescription } from '@/components/notebooks/useNotebook'
import { useCreateNotebook, useNotebooks } from '@/components/notebooks/useNotebooks'
import { ago } from '@/components/sidebar/AgentTree'
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
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { Skeleton } from '@/components/ui/skeleton'
import type { Notebook } from '@/rpc/bindings/Notebook'
import type { DeletePreview } from '@/rpc/bindings/DeletePreview'
import { useAskList } from '@/lib/ask-rpc'
import { notebookKeys, useNotebooksApi } from '@/lib/notebooks-rpc'
import { canPopout } from '@/lib/popout'
import { cn } from '@/lib/utils'

const route = getRouteApi('/notebooks')

/** `/notebooks` (144 §12.3): the Library's notebooks, live ones by default, `?archived` the archived ones; `?view=ask` (§9) the past asks; `?ask` the Ask dialog over all notebooks. */
export function NotebooksList() {
  const { archived: archivedFlag, view, ask } = route.useSearch()
  const archived = !!archivedFlag
  const navigate = useNavigate()
  const notebooks = useNotebooks(archived)
  const create = useCreateNotebook((id) => void navigate({ to: '/notebooks/$id', params: { id }, search: { new: true } }))
  const go = (search: { archived?: true; view?: 'ask'; ask?: string | true }) => void navigate({ to: '/notebooks', search, replace: search.ask !== undefined || ask !== undefined })
  const tabs = (
    <div role="tablist" className="flex items-center gap-1 text-xs">
      {(['notebooks', 'ask'] as const).map((t) => (
        <button
          key={t}
          type="button"
          role="tab"
          aria-selected={(view ?? 'notebooks') === t}
          className={cn('rounded-md px-2 py-1 text-fg-tertiary hover:bg-layer-2', (view ?? 'notebooks') === t && 'bg-layer-2 font-semibold text-fg')}
          onClick={() => go(t === 'ask' ? { view: 'ask' } : {})}
        >
          {t === 'ask' ? 'Ask' : 'Notebooks'}
        </button>
      ))}
    </div>
  )
  const dialog = <AskDialog open={ask !== undefined} onOpenChange={(o) => !o && go(view === 'ask' ? { view: 'ask' } : archived ? { archived: true } : {})} question={typeof ask === 'string' ? ask : ''} />
  if (view === 'ask')
    return (
      <Page
        actions={
          <>
            {tabs}
            <Button variant="ghost" size="sm" onClick={() => go({ view: 'ask', ask: true })}>
              <Plus /> Ask across notebooks
            </Button>
          </>
        }
      >
        <Column>
          <AskList />
        </Column>
        {dialog}
      </Page>
    )
  return (
    <Page
      actions={
        <>
          {tabs}
          <Button variant={archived ? 'secondary' : 'ghost'} size="sm" aria-pressed={archived} onClick={() => go(archived ? {} : { archived: true })}>
            <Archive /> Archived
          </Button>
          <Button variant="ghost" size="sm" disabled={create.isPending} onClick={() => create.mutate('Untitled notebook')}>
            <Plus /> New notebook
          </Button>
        </>
      }
    >
      <Column>
        {notebooks.isPending && (
          <div className="space-y-2 p-2">
            <Skeleton className="h-8 w-2/3" />
            <Skeleton className="h-8 w-1/2" />
          </div>
        )}
        {notebooks.isError && <p className="p-2 text-sm text-danger">{notebooks.error.message}</p>}
        {notebooks.data?.length === 0 && (
          <p className="p-6 text-center text-sm text-fg-tertiary">
            {archived ? (
              'No archived notebooks.'
            ) : (
              <>
                No notebooks yet. New notebook starts one; the agent's <code>notebook_create</code> lands here too.
              </>
            )}
          </p>
        )}
        <ul>{notebooks.data?.map((n) => <Row key={n.doc.id} notebook={n} archived={archived} />)}</ul>
      </Column>
      {dialog}
    </Page>
  )
}

/** Library › Notebooks › Ask (§9): past asks newest first — question, scope count, when, done/running — each to `/ask/$session`. */
function AskList() {
  const asks = useAskList()
  return (
    <>
      {asks.isPending && (
        <div className="space-y-2 p-2">
          <Skeleton className="h-8 w-2/3" />
          <Skeleton className="h-8 w-1/2" />
        </div>
      )}
      {asks.isError && <p className="p-2 text-sm text-danger">{asks.error.message}</p>}
      {asks.data?.length === 0 && <p className="p-6 text-center text-sm text-fg-tertiary">No asks yet. Ask across notebooks starts one; `/ask` in any composer too.</p>}
      <ul>
        {asks.data?.map((a) => (
          <li key={a.session} className="rounded-[10px] px-2.5 hover:bg-layer-2">
            <Link to="/ask/$session" params={{ session: a.session }} className="flex min-w-0 items-center gap-3 py-2 text-sm">
              <span className="min-w-0 flex-1 truncate font-semibold">{a.question}</span>
              <span className="shrink-0 text-xs text-fg-tertiary">
                {a.scope.length} {a.scope.length === 1 ? 'notebook' : 'notebooks'} · {ago(new Date(Number(a.created_ms)).toISOString())} · {a.done ? 'done' : 'running'}
              </span>
            </Link>
          </li>
        ))}
      </ul>
    </>
  )
}

function Row({ notebook: n, archived }: { notebook: Notebook; archived: boolean }) {
  const nb = useNotebooksApi()
  const qc = useQueryClient()
  const id = n.doc.id
  const title = n.doc.title || 'Untitled'
  const description = useDescription(id)
  const invalidate = () => void qc.invalidateQueries({ queryKey: notebookKeys.all })
  const [renaming, setRenaming] = useState<string | null>(null)
  const rename = useMutation({
    mutationFn: (t: string) => nb.notebook_update(id, t, null, null),
    onSuccess: invalidate,
    onError: (e) => toast.error(e.message),
  })
  const archive = useMutation({
    mutationFn: () => nb.notebook_update(id, null, null, !archived),
    onSuccess: () => {
      invalidate()
      toast(archived ? `Unarchived ${title}` : `Archived ${title}`)
    },
    onError: (e) => toast.error(e.message),
  })
  const [preview, setPreview] = useState<DeletePreview | null>(null)
  const previewDelete = useMutation({
    mutationFn: () => nb.notebook_delete_preview(id),
    onSuccess: setPreview,
    onError: (e) => toast.error(e.message),
  })
  const del = useMutation({
    mutationFn: (exclusive: boolean) => nb.notebook_delete(id, exclusive),
    onSuccess: () => {
      setPreview(null)
      invalidate()
      toast(`Deleted ${title}`)
    },
    onError: (e) => toast.error(e.message),
  })
  const commit = () => {
    const t = renaming?.trim()
    setRenaming(null)
    if (t && t !== n.doc.title) rename.mutate(t)
  }
  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter') e.currentTarget.blur()
    else if (e.key === 'Escape') {
      setRenaming(null)
      e.currentTarget.blur()
    }
  }
  const desc = firstLine(description.data)
  const exclusive = preview?.sources_exclusive ?? 0
  return (
    <li className="group flex items-center gap-2 rounded-[10px] px-2.5 hover:bg-layer-2">
      <NotebookIcon className="size-4 shrink-0 text-fg-tertiary" />
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
        <Link to="/notebooks/$id" params={{ id }} search={{}} className="flex min-w-0 flex-1 items-center gap-3 py-2 text-sm">
          <span className="flex min-w-0 flex-1 flex-col">
            <span className="truncate font-semibold">{title}</span>
            {desc && <span className="truncate text-xs text-fg-tertiary">{desc}</span>}
            <span className="flex items-center gap-2 text-xs text-fg-tertiary">
              <span>
                {n.sources} sources · {n.notes} notes
              </span>
              {n.project && <span className="rounded-full border border-border-subtle px-2 text-[11px]">project · {n.project}</span>}
            </span>
          </span>
          <span className="shrink-0 text-xs text-fg-tertiary">{ago(new Date(Number(n.doc.updated_ms)).toISOString())}</span>
        </Link>
      )}
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="ghost" size="icon-sm" aria-label={`Actions for ${title}`} className="opacity-0 group-hover:opacity-100 focus:opacity-100 data-[state=open]:opacity-100">
            <MoreHorizontal />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <DropdownMenuItem onSelect={() => setRenaming(n.doc.title)}>
            <Pencil /> Rename
          </DropdownMenuItem>
          <DropdownMenuItem disabled={archive.isPending} onSelect={() => archive.mutate()}>
            {archived ? (
              <>
                <ArchiveRestore /> Unarchive
              </>
            ) : (
              <>
                <Archive /> Archive
              </>
            )}
          </DropdownMenuItem>
          {canPopout() && (
            <DropdownMenuItem onSelect={() => void popOutNotebook(id)}>
              <PictureInPicture2 /> Pop out
            </DropdownMenuItem>
          )}
          <DropdownMenuItem variant="destructive" disabled={previewDelete.isPending} onSelect={() => previewDelete.mutate()}>
            <Trash2 /> Delete
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <AlertDialog open={preview !== null} onOpenChange={(o) => !o && setPreview(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete “{title}”?</AlertDialogTitle>
            <AlertDialogDescription>
              {preview?.notes ?? 0} notes stay; {exclusive} sources only in this notebook will be deleted too.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={del.isPending}>Cancel</AlertDialogCancel>
            <AlertDialogAction variant="outline" disabled={del.isPending} onClick={() => del.mutate(false)}>
              Delete, keep sources
            </AlertDialogAction>
            {exclusive > 0 && (
              <AlertDialogAction variant="destructive" disabled={del.isPending} onClick={() => del.mutate(true)}>
                Delete with its {exclusive} exclusive sources
              </AlertDialogAction>
            )}
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </li>
  )
}
