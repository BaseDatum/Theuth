import { useMutation, useQueryClient } from '@tanstack/react-query'
import { getRouteApi, useNavigate } from '@tanstack/react-router'
import { PanelLeftClose, PanelRightClose } from 'lucide-react'
import { type KeyboardEvent, type ReactNode, type Ref, useEffect, useMemo, useRef, useState } from 'react'
import { toast } from 'sonner'
import { useDocument } from '@/components/editor/useDocument'
import { SectionBand } from '@/components/nav/SectionBand'
import { SectionRow } from '@/components/nav/SectionRow'
import { ChatColumn } from '@/components/notebooks/ChatColumn'
import { NotesColumn } from '@/components/notebooks/NotesColumn'
import { firstLine, useDescription, useNotebook, useNotebookItems } from '@/components/notebooks/useNotebook'
import { useNotes } from '@/components/notes/useNotes'
import { ShareAction } from '@/components/share/ShareDialog'
import { SourcesColumn } from '@/components/sources/SourcesColumn'
import { Button } from '@/components/ui/button'
import { useIsMobile } from '@/hooks/use-mobile'
import { type Side, type Tab, layout, useNotebookColumns } from '@/lib/notebook-columns'
import { notebookKeys, useNotebooksApi } from '@/lib/notebooks-rpc'
import { cn } from '@/lib/utils'

const route = getRouteApi('/notebooks_/$id')

/** `/notebooks/$id` (144 §12.1): sources | chat | notes. `?new` = just created: the title is selected; `?tab` = the phone's open column. */
export function NotebookPage() {
  const { id } = route.useParams()
  const { new: isNew, tab } = route.useSearch()
  return <Notebook key={id} id={id} isNew={isNew === true} tab={tab ?? 'chat'} />
}

const TABS: { key: Tab; label: (sources: number, notes: number) => string }[] = [
  { key: 'sources', label: (s) => `Sources · ${s}` },
  { key: 'chat', label: () => 'Chat' },
  { key: 'notes', label: (_, n) => `Notes · ${n}` },
]

function Notebook({ id, isNew, tab }: { id: string; isNew: boolean; tab: Tab }) {
  const nb = useNotebooksApi()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const row = useNotebook(id).data
  const items = useNotebookItems(id).data
  const description = firstLine(useDescription(id).data)
  const target = useMemo(() => ({ doc: id }), [id])
  // The Notebook document itself: members for the `shared` chip and the provider `ShareAction` needs.
  const live = useDocument(target)
  const title = row?.doc.title ?? ''
  const allNotes = useNotes().data
  const notes = useMemo(() => {
    const ids = new Set(items?.filter((i) => i.kind === 'note').map((i) => i.item))
    return allNotes?.filter((n) => ids.has(n.id)) ?? []
  }, [items, allNotes])
  const sources = row?.sources ?? 0
  useEffect(() => {
    document.title = title
  }, [title])
  const rename = useMutation({
    mutationFn: (t: string) => nb.notebook_update(id, t, null, null),
    onSuccess: () => void qc.invalidateQueries({ queryKey: notebookKeys.all }),
    onError: (e) => toast.error(e.message),
  })
  // A just-created notebook (`?new`): its title is selected for typing over once the row is here; the flag leaves the URL.
  const titleInput = useRef<HTMLInputElement>(null)
  const ready = row !== undefined
  useEffect(() => {
    if (!isNew || !ready) return
    titleInput.current?.select()
    void navigate({ to: '/notebooks/$id', params: { id }, search: tab === 'chat' ? {} : { tab }, replace: true })
  }, [isNew, ready, id, tab, navigate])

  const mobile = useIsMobile()
  const collapsed = useNotebookColumns((s) => s.collapsed)
  const toggle = useNotebookColumns((s) => s.toggle)
  const l = layout(mobile, collapsed)
  const members = live.provider?.meta?.members.length ?? 0
  const chat = (composerOnly?: boolean) => <ChatColumn notebook={id} title={title || 'Untitled'} notes={notes} composerOnly={composerOnly} />

  return (
    <article className="flex min-h-0 min-w-0 flex-1 flex-col">
      <SectionBand group="library" />
      <SectionRow back={{ to: '/notebooks', label: 'Back to notebooks', className: 'md:hidden' }}>
        <TitleInput ref={titleInput} value={title} disabled={!row} onCommit={(t) => t !== title && rename.mutate(t)} />
        {description && <span className="hidden max-w-[520px] truncate text-xs text-fg-tertiary lg:inline">{description}</span>}
        <div className="ml-auto flex shrink-0 items-center gap-1">
          {row?.project && <Chip className="border-blue-700/50 text-blue-700">project · {row.project}</Chip>}
          {members > 0 && (
            <Chip className="border-emerald-700/50 text-emerald-700">
              shared · {members} {members === 1 ? 'member' : 'members'}
              {live.status.kind === 'live' && ' · live'}
            </Chip>
          )}
          <Button variant="outline" size="sm" disabled title="wave 3" className="hidden md:inline-flex">
            Ask across notebooks
          </Button>
          <Button variant="outline" size="sm" disabled title="wave 3" className="hidden md:inline-flex">
            Overviews ▾
          </Button>
          {live.provider && <ShareAction provider={live.provider} />}
          {l.mode === 'columns' && (
            <>
              <Button variant="ghost" size="icon-sm" aria-label={collapsed.sources ? 'Expand sources' : 'Collapse sources'} aria-pressed={collapsed.sources} onClick={() => toggle('sources')}>
                <PanelLeftClose />
              </Button>
              <Button variant="ghost" size="icon-sm" aria-label={collapsed.notes ? 'Expand notes' : 'Collapse notes'} aria-pressed={collapsed.notes} onClick={() => toggle('notes')}>
                <PanelRightClose />
              </Button>
            </>
          )}
        </div>
      </SectionRow>
      {l.mode === 'columns' ? (
        <div className="grid min-h-0 flex-1" style={{ gridTemplateColumns: l.template }}>
          <SideColumn side="sources" collapsed={collapsed.sources} toggle={toggle}>
            <SourcesColumn notebook={id} />
          </SideColumn>
          <div className="flex min-h-0 min-w-0 flex-col border-l border-border-subtle">{chat()}</div>
          <SideColumn side="notes" collapsed={collapsed.notes} toggle={toggle} className="border-l border-border-subtle">
            <NotesColumn notebook={id} />
          </SideColumn>
        </div>
      ) : (
        <>
          <div role="tablist" className="grid shrink-0 grid-cols-3 border-b border-border-subtle text-xs">
            {TABS.map((t) => (
              <button
                key={t.key}
                type="button"
                role="tab"
                aria-selected={tab === t.key}
                className={cn('py-2.5 text-center text-fg-tertiary', tab === t.key && 'font-semibold text-fg shadow-[inset_0_-2px_0_var(--color-primary)]')}
                onClick={() => void navigate({ to: '/notebooks/$id', params: { id }, search: t.key === 'chat' ? {} : { tab: t.key }, replace: true })}
              >
                {t.label(sources, notes.length)}
              </button>
            ))}
          </div>
          {tab === 'sources' && (
            <>
              <SourcesColumn notebook={id} />
              {chat(true)}
            </>
          )}
          {tab === 'chat' && chat()}
          {tab === 'notes' && <NotesColumn notebook={id} />}
        </>
      )}
    </article>
  )
}

function Chip({ className, children }: { className?: string; children: ReactNode }) {
  return <span className={cn('hidden whitespace-nowrap rounded-full border px-2 py-px text-[11px] sm:inline', className)}>{children}</span>
}

/** A side column, or its 12 px rail when collapsed (144 §12.1); the rail is the button that brings it back. */
function SideColumn({ side, collapsed, toggle, className, children }: { side: Side; collapsed: boolean; toggle: (side: Side) => void; className?: string; children: ReactNode }) {
  if (collapsed)
    return (
      <button type="button" className={cn('h-full w-3 bg-layer-2 hover:bg-layer-3', className)} aria-label={`Expand ${side}`} title={`Expand ${side}`} onClick={() => toggle(side)} />
    )
  return <div className={cn('flex min-h-0 min-w-0 flex-col', className)}>{children}</div>
}

function TitleInput({ ref, value, disabled, onCommit }: { ref: Ref<HTMLInputElement>; value: string; disabled: boolean; onCommit: (title: string) => void }) {
  const [draft, setDraft] = useState<string | null>(null)
  const commit = () => {
    const t = draft?.trim()
    setDraft(null)
    if (t) onCommit(t)
  }
  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter') e.currentTarget.blur()
    else if (e.key === 'Escape') {
      setDraft(null)
      e.currentTarget.blur()
    }
  }
  return (
    <input
      ref={ref}
      aria-label="Title"
      className="min-w-0 flex-1 truncate bg-transparent text-sm font-semibold outline-none placeholder:text-fg-tertiary focus:underline focus:decoration-border-subtle"
      value={draft ?? value}
      placeholder="Untitled"
      disabled={disabled}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
      onKeyDown={onKeyDown}
    />
  )
}
