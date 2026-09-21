import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Link, getRouteApi, useNavigate } from '@tanstack/react-router'
import { ChevronLeft, Copy, FileCode, History as HistoryIcon, PanelRight, Trash2 } from 'lucide-react'
import { type ComponentType, type ReactNode, useMemo, useRef, useState } from 'react'
import { toast } from 'sonner'
import { Editor as BlockEditor } from '@/components/editor/Editor'
import { useDocument } from '@/components/editor/useDocument'
import { Backlinks, Links } from '@/components/knowledge/Backlinks'
import { Editor, type EditorHandle } from '@/components/knowledge/Editor'
import { History } from '@/components/knowledge/History'
import { NewDocDialog } from '@/components/knowledge/NewDocDialog'
import { scaffold } from '@/components/knowledge/frontmatter'
import { SearchReturn } from '@/components/sidebar/SearchReturn'
import { SearchTrigger } from '@/components/sidebar/SearchTrigger'
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
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { Skeleton } from '@/components/ui/skeleton'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { useIsMobile } from '@/hooks/use-mobile'
import { useSearchFocus } from '@/hooks/use-search-focus'
import { isMacDesktop } from '@/lib/hotkeys'
import { queryKeys, useApi } from '@/lib/rpc'
import { isSearchDestination, useSearchOrigin, useSearchState } from '@/lib/search'
import type { Concept } from '@/rpc/bindings/Concept'
import type { HistorySearchHit } from '@/rpc/bindings/HistorySearchHit'
import type { MatchRange } from '@/rpc/bindings/MatchRange'
import { cn } from 'cn'
import { MeetingPill } from '@/components/meeting/MeetingPill'

const route = getRouteApi('/knowledge/$')

/**
 * `/knowledge/$`: one concept. `?draft=` is a not-yet-written one opened straight in the editor;
 * `?change=` + `?snapshot=` read that historical text instead of the current one (49 §7);
 * `?history=true` / `?change=` open the History sheet, so the sheet is URL state and the composer's
 * `/knowledge history <id>` lands with it open.
 */
export function Document() {
  const { _splat: id = '' } = route.useParams()
  const { draft, history, change, snapshot } = route.useSearch()
  return (
    <Doc key={id} id={id} draft={draft} history={history === true} change={change} snapshot={snapshot} />
  )
}

/** Keyed by id so editing/dialog state never leaks between concepts. */
function Doc({
  id,
  draft,
  history,
  change,
  snapshot,
}: {
  id: string
  draft: string | undefined
  history: boolean
  change: string | undefined
  snapshot: string | undefined
}) {
  const api = useApi()
  const navigate = route.useNavigate()
  const [editing, setEditing] = useState(false)
  const raw = useRef<EditorHandle>(null)
  const [missing, setMissing] = useState<{ id: string; title: string } | null>(null)
  const [railOpen, setRailOpen] = useState(true)
  const mobile = useIsMobile()
  const origin = useSearchOrigin()
  const historical = draft === undefined && change !== undefined && snapshot !== undefined
  const doc = useQuery({
    queryKey: queryKeys.knowledgeDoc(id),
    queryFn: () => api.knowledge_get(id),
    enabled: draft === undefined && !historical,
  })
  // A `knowledge` hit lands on the editor column once the current text rendered (49 §7); refetches and ordinary visits leave focus alone.
  const column = useSearchFocus<HTMLDivElement>(
    isSearchDestination(origin, { category: 'knowledge', id }) ? origin : null,
    doc.isSuccess && !editing,
  )
  // In the historical reader `change` only names the snapshot's change; the sheet is `history` alone.
  const sheetOpen = historical ? history : history || change !== undefined
  const setSheetOpen = (open: boolean) =>
    navigate({
      to: '/knowledge/$',
      params: { _splat: id },
      replace: true,
      search: ({ history: _history, change: c, snapshot: s, ...rest }) =>
        open ? { ...rest, change: c, snapshot: s, history: true } : historical ? { ...rest, change: c, snapshot: s } : rest,
    })
  const sheet = (
    <History id={id} open={sheetOpen} onOpenChange={setSheetOpen} initialCommit={change} />
  )
  const createMissing = (
    <NewDocDialog
      open={missing !== null}
      onOpenChange={(o) => !o && setMissing(null)}
      initial={missing ?? undefined}
    />
  )

  if (draft !== undefined) {
    return (
      <Frame id={id}>
        <Editor
          id={id}
          initial={scaffold(id, draft)}
          onSaved={() =>
            navigate({
              to: '/knowledge/$',
              params: { _splat: id },
              search: {},
              replace: true,
            })
          }
          onCancel={() => navigate({ to: '/knowledge' })}
        />
      </Frame>
    )
  }
  if (historical) {
    return (
      <Frame
        id={id}
        actions={<Action label="Changes" icon={HistoryIcon} onClick={() => setSheetOpen(true)} />}
      >
        <Historical id={id} change={change} snapshot={snapshot} />
        {sheet}
      </Frame>
    )
  }
  if (doc.isPending) {
    return (
      <Frame id={id}>
        <div className="mx-auto w-full max-w-[768px] space-y-3 p-6">
          <Skeleton className="h-8 w-1/2" />
          <Skeleton className="h-4 w-3/4" />
          <Skeleton className="h-4 w-2/3" />
        </div>
        {sheet}
      </Frame>
    )
  }
  if (doc.isError) {
    return (
      <Frame id={id}>
        <div className="m-auto max-w-full px-6 text-center text-sm">
          <p className="text-danger wrap-anywhere">{doc.error.message}</p>
          <div className="mt-2 flex flex-wrap justify-center gap-1">
            <Button variant="ghost" size="sm" onClick={() => setMissing({ id, title: id })}>
              Create {id}
            </Button>
            {/* A deleted concept's commits are still there; the sheet can restore it from one. */}
            <Button variant="ghost" size="sm" onClick={() => setSheetOpen(true)}>
              <HistoryIcon />
              History
            </Button>
            <SearchReturn />
          </div>
          {createMissing}
          {sheet}
        </div>
      </Frame>
    )
  }
  const concept = doc.data
  const rail = (
    <>
      <Backlinks ids={concept.cited_by} />
      <Links
        links={concept.links}
        onMissing={(target) => setMissing({ id: target, title: target })}
      />
    </>
  )
  return (
    <Frame
      id={id}
      actions={
        <Actions
          concept={concept}
          railOpen={railOpen}
          onRaw={() => setEditing(true)}
          onHistory={() => setSheetOpen(true)}
          onToggleRail={() => setRailOpen((v) => !v)}
        />
      }
    >
      <div className="flex min-h-0 flex-1">
        <div ref={column} tabIndex={-1} className="flex min-w-0 flex-1 flex-col outline-none">
          <SearchReturn className="mx-auto mt-2 w-full max-w-[768px] px-4" />
          <Live id={id} />
          {/* Under `md` the links rail has no room beside the body; it follows the body instead, still toggled by the same action. */}
          {mobile && railOpen && (
            <section className="max-h-[40%] shrink-0 overflow-y-auto border-t border-border-subtle">{rail}</section>
          )}
        </div>
        {!mobile && railOpen && (
          <aside className="w-[260px] shrink-0 overflow-y-auto border-l border-border-subtle">
            {rail}
          </aside>
        )}
      </div>
      {/* "Open raw": the whole file in CodeMirror (59 §9); a save is a `knowledge_write`, which re-imports into the editor by block diff.
          Escape, outside clicks and the X go through the editor's own leave (it asks when dirty); Radix must not close on its own. */}
      <Sheet open={editing} onOpenChange={(open) => (open ? setEditing(true) : raw.current?.leave())}>
        <SheetContent
          className="flex w-full flex-col gap-0 sm:w-[720px] sm:max-w-[720px]"
          onEscapeKeyDown={(e) => {
            e.preventDefault()
            raw.current?.leave()
          }}
          onInteractOutside={(e) => {
            e.preventDefault()
            raw.current?.leave()
          }}
          onOpenAutoFocus={(e) => e.preventDefault()}
        >
          <SheetHeader className="border-b border-border-subtle">
            <SheetTitle className="font-mono text-sm">{id}</SheetTitle>
            <SheetDescription>Raw markdown and header; ⌘S writes the file.</SheetDescription>
          </SheetHeader>
          {editing && (
            <Editor
              ref={raw}
              id={id}
              initial={concept.text}
              onSaved={() => setEditing(false)}
              onCancel={() => setEditing(false)}
            />
          )}
        </SheetContent>
      </Sheet>
      {createMissing}
      {sheet}
    </Frame>
  )
}

/** The current revision in BlockNote over its live document (59 §7.1: created from the file on first open). */
function Live({ id }: { id: string }) {
  const binding = useMemo(() => ({ knowledge: id }), [id])
  const { provider, status, error } = useDocument(binding)
  if (error) {
    return <p className="m-auto max-w-full px-6 text-center text-sm text-danger wrap-anywhere">{error.message}</p>
  }
  if (!provider) {
    return (
      <div className="mx-auto w-full max-w-[768px] space-y-3 p-6">
        <Skeleton className="h-8 w-1/2" />
        <Skeleton className="h-4 w-3/4" />
        <Skeleton className="h-4 w-2/3" />
      </div>
    )
  }
  return <BlockEditor provider={provider} status={status} />
}

/** 52 px bar (id left, actions right) over the pane body. Under `md` a back chevron returns to the tree and a search trigger trails the actions (49 §5.1). */
function Frame({
  id,
  actions,
  children,
}: {
  id: string
  actions?: ReactNode
  children: ReactNode
}) {
  return (
    <article className="flex min-h-0 min-w-0 flex-1 flex-col">
      <header
        data-tauri-drag-region={isMacDesktop() || undefined}
        className="flex h-[52px] shrink-0 items-center gap-1 border-b border-border-subtle px-3"
      >
        <Button
          asChild
          variant="ghost"
          size="icon-sm"
          className="md:hidden"
          aria-label="Back to knowledge"
        >
          <Link to="/knowledge">
            <ChevronLeft />
          </Link>
        </Button>
        <code className="min-w-0 flex-1 truncate font-mono text-xs text-fg-tertiary">{id}</code>
        {actions}
        <SearchTrigger className="md:hidden" />
        <MeetingPill />
      </header>
      {children}
    </article>
  )
}

function Actions({
  concept,
  railOpen,
  onRaw,
  onHistory,
  onToggleRail,
}: {
  concept: Concept
  railOpen: boolean
  onRaw: () => void
  onHistory: () => void
  onToggleRail: () => void
}) {
  const api = useApi()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const id = concept.meta.id
  const [deleting, setDeleting] = useState(false)
  const del = useMutation({
    mutationFn: () => api.knowledge_delete(id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: queryKeys.knowledgeList })
      qc.removeQueries({ queryKey: queryKeys.knowledgeDoc(id) })
      toast(`Deleted ${id}`)
      navigate({ to: '/knowledge' })
    },
    onError: (e) => toast.error(e.message),
  })
  const cited = concept.cited_by.length
  return (
    <>
      <Action label="Open raw" icon={FileCode} onClick={onRaw} />
      <Action label="History" icon={HistoryIcon} onClick={onHistory} />
      <Action
        label="Copy id"
        icon={Copy}
        onClick={() =>
          navigator.clipboard.writeText(id).then(
            () => toast('Copied'),
            (e: unknown) => toast.error(String(e)),
          )
        }
      />
      <Action label="Delete" icon={Trash2} onClick={() => setDeleting(true)} />
      <Action
        label={railOpen ? 'Hide links' : 'Show links'}
        icon={PanelRight}
        onClick={onToggleRail}
        pressed={railOpen}
      />
      <AlertDialog open={deleting} onOpenChange={setDeleting}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete {id}?</AlertDialogTitle>
            <AlertDialogDescription>
              {cited === 0
                ? 'Nothing links to this document.'
                : `${cited} ${cited === 1 ? 'document links' : 'documents link'} here; those links will break.`}{' '}
              The file stays in history.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              disabled={del.isPending}
              onClick={() => del.mutate()}
            >
              Delete
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  )
}

function Action({
  label,
  icon: Icon,
  onClick,
  pressed,
}: {
  label: string
  icon: ComponentType<{ className?: string }>
  onClick: () => void
  pressed?: boolean
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          className="size-11 rounded-xl"
          aria-label={label}
          aria-pressed={pressed}
          onClick={onClick}
        >
          <Icon className={cn('size-[18px]', pressed === false && 'text-fg-tertiary')} />
        </Button>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  )
}

/**
 * The complete text of `id` as committed at `snapshot`, read-only (49 §7): never `knowledge_get`, so a
 * deleted concept or a commit outside the newest history page reads the same way. `change` is the
 * commit whose diff the text belongs to: After when it is the snapshot, Before when the snapshot is its parent.
 */
function Historical({ id, change, snapshot }: { id: string; change: string; snapshot: string }) {
  const api = useApi()
  const origin = useSearchOrigin()
  const view = useSearchState((s) => s.view)
  const side = snapshot === change ? 'After' : 'Before'
  const text = useQuery({
    queryKey: queryKeys.knowledgeReadRevision(id, snapshot),
    queryFn: () => api.knowledge_read_revision(id, snapshot),
    staleTime: Number.POSITIVE_INFINITY,
  })
  const meta = useQuery({
    queryKey: queryKeys.knowledgeRevision(change, id),
    queryFn: () => api.knowledge_revision(change, id),
    staleTime: Number.POSITIVE_INFINITY,
  })
  const matched = isSearchDestination(origin, { category: 'knowledge_history', id, commit: change, snapshot }) ? origin : null
  // `leave` stored the view and origin together, so when this page is the origin's destination the
  // displayed groups hold the very hit that was activated; its server snippet/spans are the highlight.
  const hit = matched
    ? view?.groups
        .flatMap((g) => (g.category === 'knowledge_history' ? g.page.hits : []))
        .find((h) => h.id === id && h.commit === change && h.snapshot_commit === snapshot)
    : undefined
  // The activation lands on the snapshot heading, which is there before the text (49 §7).
  const heading = useSearchFocus<HTMLHeadingElement>(matched, true)
  return (
    <div className="min-w-0 flex-1 overflow-y-auto">
      <div className="mx-auto w-full max-w-[768px] px-6 py-4">
        <SearchReturn className="-ml-2 mb-2" />
        <header className="mb-6 border-b border-border-subtle pb-4">
          <div className="flex flex-wrap items-center gap-2">
            <h1 ref={heading} tabIndex={-1} className="text-[24px] leading-8 font-semibold outline-none">
              Historical version
            </h1>
            <Badge variant="outline">{side}</Badge>
          </div>
          <p className="mt-1 text-sm text-fg-secondary wrap-anywhere">
            {side === 'After' ? 'As committed by ' : 'As it was before '}
            <code className="font-mono text-xs">{change.slice(0, 7)}</code>
            {meta.data && ` · ${meta.data.change.time} · ${meta.data.change.message}`}
            {meta.isError && (
              <span className="text-danger"> · change unavailable: {meta.error.message}</span>
            )}
          </p>
          <p className="mt-1 text-xs text-fg-tertiary wrap-anywhere">
            Read-only snapshot at <code className="font-mono">{snapshot}</code>. The current document may
            differ or no longer exist; Changes shows the diff, Restore lives there.
          </p>
        </header>
        {text.isPending ? (
          <div className="space-y-3">
            <Skeleton className="h-4 w-3/4" />
            <Skeleton className="h-4 w-2/3" />
            <Skeleton className="h-4 w-1/2" />
          </div>
        ) : text.isError ? (
          <div role="alert" className="text-sm">
            <p className="text-danger wrap-anywhere">{text.error.message}</p>
            <Button
              variant="outline"
              size="sm"
              className="mt-2 pointer-coarse:min-h-11"
              onClick={() => void text.refetch()}
            >
              Retry
            </Button>
          </div>
        ) : (
          <Raw text={text.data.text} hit={hit} />
        )}
      </div>
    </div>
  )
}

/**
 * Verbatim committed text. When opened from a search, the exact server `TextMatch` of this hit heads it
 * (49 §7): the same snippet and spans the results row showed, never a client-side re-match of the query.
 */
function Raw({ text, hit }: { text: string; hit: HistorySearchHit | undefined }) {
  return (
    <>
      {hit && (
        <section aria-label="Matched text" className="mb-4 rounded-xl bg-layer-1 px-4 py-3">
          <div className="flex items-center gap-2">
            <h2 className="text-xs font-semibold text-fg-secondary">Matched text</h2>
            <span className="text-xs text-fg-tertiary">{hit.match.field}</span>
          </div>
          <pre className="mt-2 rounded-lg bg-layer-2 p-3 font-sans text-[13px] leading-6 whitespace-pre-wrap wrap-anywhere">
            <Marked text={hit.match.snippet} ranges={hit.match.ranges} />
          </pre>
        </section>
      )}
      <pre className="font-mono text-sm leading-6 whitespace-pre-wrap wrap-anywhere">{text}</pre>
    </>
  )
}

/** `ranges` are `[start, end)` char offsets (not UTF-16 units), so slice the code-point array. */
function Marked({ text, ranges }: { text: string; ranges: MatchRange[] }) {
  const chars = Array.from(text)
  const out: ReactNode[] = []
  let i = 0
  for (const r of ranges) {
    if (r.start > i) out.push(chars.slice(i, r.start).join(''))
    out.push(
      <mark key={r.start} className="rounded-xs bg-accent font-semibold text-foreground">
        {chars.slice(r.start, r.end).join('')}
      </mark>,
    )
    i = r.end
  }
  out.push(chars.slice(i).join(''))
  return <>{out}</>
}
