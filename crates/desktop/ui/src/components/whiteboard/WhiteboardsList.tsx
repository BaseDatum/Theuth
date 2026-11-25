import { useMutation, useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import { MoreHorizontal, Pencil, Plus, Presentation, Trash2 } from 'lucide-react'
import { type KeyboardEvent, useState } from 'react'
import { SectionBand } from '@/components/nav/SectionBand'
import { SectionRow } from '@/components/nav/SectionRow'
import { ago } from '@/components/sidebar/AgentTree'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { Skeleton } from '@/components/ui/skeleton'
import { notify } from '@/lib/notify'
import { useApi } from '@/lib/rpc'
import type { DocSummary } from '@/rpc/bindings/method.agent.output'
import { useCreateWhiteboard, useWhiteboards, whiteboardsKey } from './useWhiteboards'

export const WHITEBOARD_BLURB = 'A canvas for thinking with the agent: mocks, diagrams, notes and Pencil marks.'

/** 218 `/whiteboards`: every whiteboard, newest update first; New whiteboard; per-row Rename (inline) and Delete. */
export function WhiteboardsList() {
  const navigate = useNavigate()
  const list = useWhiteboards()
  const createWhiteboard = useCreateWhiteboard()
  const create = useMutation({
    mutationFn: () => createWhiteboard(),
    onSuccess: (id) => void navigate({ to: '/whiteboards/$id', params: { id } }),
    onError: (e) => notify.error(e.message),
  })
  return (
    <article className="flex min-h-0 min-w-0 flex-1 flex-col">
      <SectionBand group="library" />
      <SectionRow>
        <h1 className="text-sm font-semibold">Whiteboards</h1>
        <Button variant="ghost" size="sm" className="ml-auto" disabled={create.isPending} onClick={() => create.mutate()}>
          <Plus /> New whiteboard
        </Button>
      </SectionRow>
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto w-full max-w-[768px] p-3">
          {list.isPending && (
            <div className="space-y-2 p-2">
              <Skeleton className="h-8 w-2/3" />
              <Skeleton className="h-8 w-1/2" />
            </div>
          )}
          {list.isError && <p className="p-2 text-sm text-danger">{list.error.message}</p>}
          {list.data?.length === 0 && (
            <p className="p-6 text-center text-sm text-fg-tertiary">
              No whiteboards yet. {WHITEBOARD_BLURB} New whiteboard starts one.
            </p>
          )}
          <ul>
            {list.data?.map((w) => <Row key={w.id} wb={w} />)}
          </ul>
        </div>
      </div>
    </article>
  )
}

function Row({ wb }: { wb: DocSummary }) {
  const api = useApi()
  const qc = useQueryClient()
  const [renaming, setRenaming] = useState<string | null>(null)
  const rename = useMutation({
    mutationFn: (t: string) => api.doc_rename(wb.id, t),
    onSuccess: () => void qc.invalidateQueries({ queryKey: whiteboardsKey }),
    onError: (e) => notify.error(e.message),
  })
  const del = useMutation({
    mutationFn: () => api.doc_delete(wb.id),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: whiteboardsKey })
      notify(`Deleted ${wb.title || 'the whiteboard'}`)
    },
    onError: (e) => notify.error(e.message),
  })
  const commit = () => {
    const t = renaming?.trim()
    setRenaming(null)
    if (t && t !== wb.title) rename.mutate(t)
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
      <Presentation className="size-4 shrink-0 text-fg-tertiary" />
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
        <Link to="/whiteboards/$id" params={{ id: wb.id }} className="flex min-w-0 flex-1 items-baseline gap-2 py-2 text-sm">
          <span className="min-w-0 flex-1 truncate">{wb.title || 'Untitled whiteboard'}</span>
          <span className="shrink-0 text-xs text-fg-tertiary">{ago(new Date(Number(wb.updated_ms)).toISOString())}</span>
        </Link>
      )}
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="ghost" size="icon-sm" aria-label={`Actions for ${wb.title || 'Untitled whiteboard'}`} className="opacity-0 group-hover:opacity-100 focus:opacity-100 data-[state=open]:opacity-100">
            <MoreHorizontal />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <DropdownMenuItem onSelect={() => setRenaming(wb.title)}>
            <Pencil /> Rename
          </DropdownMenuItem>
          <DropdownMenuItem variant="destructive" disabled={del.isPending} onSelect={() => del.mutate()}>
            <Trash2 /> Delete
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </li>
  )
}
