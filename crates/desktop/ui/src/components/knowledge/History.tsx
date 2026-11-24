import { useQuerySupport } from '@/rpc/capabilities'
import { keepPreviousData, useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { ChevronLeft } from 'lucide-react'
import { useState } from 'react'
import { notify as toast } from '@/lib/notify'
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
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { Skeleton } from '@/components/ui/skeleton'
import { useSearchFocus } from '@/hooks/use-search-focus'
import { queryKeys, useApi } from '@/lib/rpc'
import { isSearchDestination, useSearchOrigin } from '@/lib/search'
import type { Change } from "@/rpc/bindings/method.knowledge.output"
import type { Revision } from "@/rpc/bindings/method.knowledge.output"
import { cn } from 'cn'

type ChangedFile = Revision['files'][number]

type Props = {
  /** One concept, or `null` for the whole bundle. */
  id: string | null
  open: boolean
  onOpenChange: (open: boolean) => void
  /** Select this commit (full or abbreviated id) on open; fetched directly, it need not be on the first page. */
  initialCommit?: string
}

const PAGE = 20

/** Same commit whether the list's 7-char abbreviation or a route's full id names it. */
function sameCommit(a: string, b: string): boolean {
  return a.startsWith(b) || b.startsWith(a)
}

/** Commits touching `id` (or every commit), any of them selectable; the selection shows the concept files it changed with unified diffs. Restore rewrites a file from the commit holding the displayed text after a confirm. */
export function History({ id, open, onOpenChange, initialCommit }: Props) {
  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      {/* Radix unmounts the content when closed, so every open starts at the sheet's own scope. */}
      <SheetContent className="flex w-full flex-col gap-0 sm:w-[560px] sm:max-w-[560px]">
        <Panel id={id} initialCommit={initialCommit} />
      </SheetContent>
    </Sheet>
  )
}

/** The sheet's scope plus one drill-in: a changed document's own history, Back returns. */
function Panel({ id, initialCommit }: { id: string | null; initialCommit?: string }) {
  const [scope, setScope] = useState(id)
  return (
    <>
      <SheetHeader className="pr-12">
        <SheetTitle className="flex items-center gap-1">
          {scope !== id && (
            <Button
              variant="ghost"
              size="icon-sm"
              className="-ml-2"
              aria-label={`Back to ${id ?? 'all documents'}`}
              onClick={() => setScope(id)}
            >
              <ChevronLeft />
            </Button>
          )}
          History
        </SheetTitle>
        <SheetDescription className="font-mono text-xs wrap-anywhere">
          {scope ?? 'All documents'}
        </SheetDescription>
      </SheetHeader>
      {/* Keyed so paging, selection and the restore prompt never survive a scope change. */}
      <Scoped
        key={scope ?? ''}
        id={scope}
        initialCommit={scope === id ? initialCommit : undefined}
        onScope={setScope}
      />
    </>
  )
}

function Scoped({
  id,
  initialCommit,
  onScope,
}: {
  id: string | null
  initialCommit: string | undefined
  onScope: (id: string) => void
}) {
  const canQuery1 = useQuerySupport(["knowledge_history"])
  const canQuery1_2 = useQuerySupport(["knowledge_revision"])
  const api = useApi()
  const qc = useQueryClient()
  const origin = useSearchOrigin()
  const [limit, setLimit] = useState(PAGE)
  const [selected, setSelected] = useState<string | null>(initialCommit ?? null)
  const [restoring, setRestoring] = useState<{ id: string; commit: string; change: Change } | null>(
    null,
  )
  // One extra row tells whether an older page exists without a count RPC.
  const history = useQuery({ enabled: canQuery1,
    queryKey: queryKeys.knowledgeHistory(id, limit),
    queryFn: () => api.knowledge_history(id, limit + 1),
    placeholderData: keepPreviousData,
  })
  const all = history.data?.changes ?? []
  const changes = all.length > limit ? all.slice(0, limit) : all
  const more = all.length > limit
  // A direct selection reads its revision straight away; the list only has to contain it for highlighting.
  const current = selected ?? changes[0]?.commit ?? null
  const listed = current === null || changes.some((c) => sameCommit(c.commit, current))
  const revision = useQuery({
    queryKey: queryKeys.knowledgeRevision(current ?? '', id),
    queryFn: () => api.knowledge_revision(current ?? '', id),
    enabled: canQuery1_2 && (current !== null),
    staleTime: Number.POSITIVE_INFINITY,
  })
  // A commit-metadata hit (`snapshot: null`) opens this sheet on its commit: focus lands on the selected commit's
  // detail once it rendered, not on the (possibly deleted) document behind the sheet (49 §7).
  const matched =
    id !== null && initialCommit !== undefined && isSearchDestination(origin, { category: 'knowledge_history', id, commit: initialCommit, snapshot: null })
      ? origin
      : null
  const detail = useSearchFocus<HTMLElement>(matched, revision.isSuccess)
  const restore = useMutation({
    mutationFn: (r: { id: string; commit: string }) => api.knowledge_restore(r.id, r.commit),
    onSuccess: (c, r) => {
      qc.invalidateQueries({ queryKey: queryKeys.knowledgeDoc(r.id) })
      qc.invalidateQueries({ queryKey: queryKeys.knowledgeList })
      qc.invalidateQueries({ queryKey: queryKeys.knowledgeHistories })
      toast(`Restored ${r.id} from ${c.commit}`)
      setRestoring(null)
    },
  })
  const closeRestore = () => {
    setRestoring(null)
    restore.reset()
  }

  return (
    <div className="min-h-0 flex-1 overflow-y-auto px-4 pb-4">
      {history.isPending ? (
        <div className="space-y-2">
          <Skeleton className="h-11" />
          <Skeleton className="h-11" />
          <Skeleton className="h-11" />
        </div>
      ) : history.isError ? (
        <Failed message={history.error.message} onRetry={() => void history.refetch()} />
      ) : changes.length === 0 && current === null ? (
        <p className="py-4 text-sm text-fg-tertiary">No history yet.</p>
      ) : (
        <>
          <ul
            aria-label="Commits"
            className="max-h-[35dvh] overflow-y-auto divide-y divide-border-subtle px-2"
          >
            {changes.map((c) => {
              const active = current !== null && sameCommit(c.commit, current)
              return (
                <li key={c.commit}>
                  <button
                    type="button"
                    aria-pressed={active}
                    onClick={() => setSelected(c.commit)}
                    className={cn(
                      '-mx-2 flex w-[calc(100%+1rem)] min-h-11 flex-wrap items-baseline gap-x-3 gap-y-0.5 rounded-lg px-2 py-2 text-left text-sm outline-none hover:bg-layer-2 focus-visible:ring-[3px] focus-visible:ring-ring/50',
                      active && 'bg-layer-2',
                    )}
                  >
                    <code className="font-mono text-xs text-fg-tertiary">{c.commit}</code>
                    <span className="text-xs text-fg-tertiary tabular-nums">{c.time}</span>
                    <span className="w-full min-w-0 wrap-anywhere">{c.message}</span>
                  </button>
                </li>
              )
            })}
          </ul>
          {more && (
            <Button
              variant="ghost"
              size="sm"
              className="mt-1 w-full pointer-coarse:min-h-11"
              disabled={history.isFetching}
              onClick={() => setLimit((l) => l + PAGE)}
            >
              {history.isFetching ? 'Loading…' : 'Older'}
            </Button>
          )}
          {!listed && (
            <p className="mt-2 text-xs text-fg-tertiary">
              The selected change is older than the listed ones.
            </p>
          )}
          {current !== null && (
            <section
              ref={detail}
              tabIndex={-1}
              aria-label="Selected commit"
              className="mt-4 border-t border-border-subtle pt-4 outline-none"
            >
              {revision.isPending ? (
                <div className="space-y-2">
                  <Skeleton className="h-5 w-2/3" />
                  <Skeleton className="h-24" />
                </div>
              ) : revision.isError ? (
                <Failed message={revision.error.message} onRetry={() => void revision.refetch()} />
              ) : (
                <Detail
                  revision={revision.data}
                  scope={id}
                  onScope={onScope}
                  onRestore={(file, commit) => {
                    restore.reset()
                    setRestoring({ id: file.id, commit, change: revision.data.change })
                  }}
                />
              )}
            </section>
          )}
        </>
      )}
      <AlertDialog open={restoring !== null} onOpenChange={(o) => !o && closeRestore()}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle className="wrap-anywhere">
              Restore {restoring?.id} from {restoring?.commit}?
            </AlertDialogTitle>
            <AlertDialogDescription className="wrap-anywhere">
              {restoring !== null &&
                (restoring.commit === restoring.change.commit
                  ? `Rewritten as of ${restoring.change.time} (${restoring.change.message}).`
                  : `Rewritten as it was before ${restoring.change.commit} deleted it (${restoring.change.time}, ${restoring.change.message}).`)}{' '}
              The current version stays in history.
            </AlertDialogDescription>
          </AlertDialogHeader>
          {restore.isError && (
            <p role="alert" className="text-sm text-danger wrap-anywhere">
              {restore.error.message}
            </p>
          )}
          <AlertDialogFooter>
            <AlertDialogCancel disabled={restore.isPending}>Cancel</AlertDialogCancel>
            {/* Stays open until the write lands: a failure shows here instead of vanishing. */}
            <AlertDialogAction
              disabled={restore.isPending}
              onClick={(e) => {
                e.preventDefault()
                if (restoring) restore.mutate({ id: restoring.id, commit: restoring.commit })
              }}
            >
              {restore.isPending ? 'Restoring…' : restore.isError ? 'Retry' : 'Restore'}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}

function Failed({ message, onRetry }: { message: string; onRetry: () => void }) {
  return (
    <div role="alert" className="py-4 text-sm">
      <p className="text-danger wrap-anywhere">{message}</p>
      <Button
        variant="outline"
        size="sm"
        className="mt-2 pointer-coarse:min-h-11"
        onClick={onRetry}
      >
        Retry
      </Button>
    </div>
  )
}

const STATUS: Record<ChangedFile['status'], string> = {
  added: 'border-success-border bg-success-bg text-success',
  modified: 'border-info-border bg-info-bg text-info',
  deleted: 'border-danger-border bg-danger-bg text-danger',
}

/** The selected commit: metadata, then each concept file it changed with its diff. Restore targets the commit holding the shown text: the commit itself, or its parent for a deletion. */
function Detail({
  revision,
  scope,
  onScope,
  onRestore,
}: {
  revision: Revision
  scope: string | null
  onScope: (id: string) => void
  onRestore: (file: ChangedFile, commit: string) => void
}) {
  const { change, parent, files } = revision
  return (
    <>
      <div className="text-sm">
        <div className="flex flex-wrap items-baseline gap-x-3 text-xs text-fg-tertiary">
          <code className="font-mono">{change.commit}</code>
          <span className="tabular-nums">{change.time}</span>
        </div>
        <p className="mt-1 wrap-anywhere">{change.message}</p>
        <p className="mt-1 text-xs text-fg-tertiary wrap-anywhere">
          {parent === null ? (
            'Root commit (compared with an empty bundle)'
          ) : (
            <>
              Parent <code className="font-mono">{parent}</code>
            </>
          )}
        </p>
      </div>
      {files.length === 0 ? (
        <p className="mt-3 text-sm text-fg-tertiary">No documents changed in this commit.</p>
      ) : (
        <ul aria-label="Changed documents" className="mt-3 space-y-4">
          {files.map((f) => {
            const from = f.status === 'deleted' ? parent : change.commit
            return (
              <li key={f.id}>
                <div className="flex flex-wrap items-center gap-2">
                  <Badge variant="outline" className={STATUS[f.status]}>
                    {f.status}
                  </Badge>
                  {f.id === scope ? (
                    <code className="min-w-0 flex-1 font-mono text-xs wrap-anywhere">{f.id}</code>
                  ) : (
                    <button
                      type="button"
                      aria-label={`History of ${f.id}`}
                      onClick={() => onScope(f.id)}
                      className="min-w-0 flex-1 rounded-sm text-left font-mono text-xs wrap-anywhere underline decoration-dotted underline-offset-2 outline-none hover:text-foreground focus-visible:ring-[3px] focus-visible:ring-ring/50 pointer-coarse:min-h-11"
                    >
                      {f.id}
                    </button>
                  )}
                  {from !== null && (
                    <Button
                      variant="ghost"
                      size="xs"
                      className="pointer-coarse:min-h-11"
                      aria-label={`Restore ${f.id} from ${from}`}
                      onClick={() => onRestore(f, from)}
                    >
                      Restore
                    </Button>
                  )}
                </div>
                <UnifiedDiff diff={f.diff} />
              </li>
            )
          })}
        </ul>
      )}
    </>
  )
}

/** Colour a unified diff by line prefix; wraps instead of scrolling so a phone-width sheet never overflows. */
function UnifiedDiff({ diff }: { diff: string }) {
  if (diff === '') return null
  const lines = diff.replace(/\n$/, '').split('\n')
  return (
    <pre className="mt-2 rounded-xl bg-layer-2 py-2 font-mono text-xs leading-5">
      {lines.map((l, i) => (
        // Positional rows of one diff: the index is the identity.
        <div key={i} className={cn('px-3 whitespace-pre-wrap wrap-anywhere', rowClass(l))}>
          {l}
        </div>
      ))}
    </pre>
  )
}

function rowClass(line: string): string {
  if (line.startsWith('+++') || line.startsWith('---')) return 'text-fg-tertiary'
  if (line.startsWith('@@')) return 'text-info'
  if (line.startsWith('+')) return 'bg-success-bg text-success'
  if (line.startsWith('-')) return 'bg-danger-bg text-danger'
  return 'text-fg-secondary'
}
