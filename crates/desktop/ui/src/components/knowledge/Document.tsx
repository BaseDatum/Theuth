import { useQuerySupport } from '@/rpc/capabilities'
import { code } from '@streamdown/code'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { getRouteApi, useNavigate } from '@tanstack/react-router'
import { openUrl } from '@tauri-apps/plugin-opener'
import { FRAGMENT } from '@theuth/documents/schema'
import { Copy, FileCode, History as HistoryIcon, MessageSquarePlus, PanelRight, Sparkles, Trash2, Undo2 } from 'lucide-react'
import { type MouseEvent, type ReactNode, useEffect, useMemo, useRef, useState } from 'react'
import { toast } from 'sonner'
import { type Components, Streamdown } from 'streamdown'
import { prefillDraft } from '@/components/chat/Composer'
import { Action, DocumentPage, Frame } from '@/components/editor/DocumentPage'
import { type LiveDocument, useDocument } from '@/components/editor/useDocument'
import { Backlinks, Links } from '@/components/knowledge/Backlinks'
import { Editor, type EditorHandle } from '@/components/knowledge/Editor'
import { History } from '@/components/knowledge/History'
import { NewDocDialog } from '@/components/knowledge/NewDocDialog'
import { scaffold, stripFrontmatter } from '@/components/knowledge/frontmatter'
import { ChatPicker } from '@/components/notes/ChatPicker'
import { SearchReturn } from '@/components/sidebar/SearchReturn'
import { StatusBadge } from '@/components/knowledge/Tree'
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
import { useSearchFocus } from '@/hooks/use-search-focus'
import { type AttachRef, toRef } from '@/lib/attach'
import type { EngineProvider } from '@/lib/doc/provider'
import { classify, resolveLink } from '@/lib/resolveLink'
import { queryKeys, useApi } from '@/lib/rpc'
import { isSearchDestination, useSearchOrigin, useSearchState } from '@/lib/search'
import type { Concept } from "@/rpc/bindings/method.knowledge.output"
import type { HistorySearchHit } from "@/rpc/bindings/method.rpc_2dcompose.output"
import type { MatchRange } from "@/rpc/bindings/method.artifact.output"
import { cn } from 'cn'

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
  const canQuery1 = useQuerySupport(["knowledge_get"])
  const api = useApi()
  const navigate = route.useNavigate()
  const [editing, setEditing] = useState(false)
  const raw = useRef<EditorHandle>(null)
  const [missing, setMissing] = useState<{ id: string; title: string } | null>(null)
  const [railOpen, setRailOpen] = useState(true)
  const [attaching, setAttaching] = useState(false)
  const origin = useSearchOrigin()
  const historical = draft === undefined && change !== undefined && snapshot !== undefined
  const binding = useMemo(() => ({ knowledge: id }), [id])
  const live = useDocument(binding)
  const doc = useQuery({
    queryKey: queryKeys.knowledgeDoc(id),
    queryFn: () => api.knowledge_get(id),
    enabled: canQuery1 && (draft === undefined && !historical),
  })
  const unavailable = useUnavailable(live, doc.data !== undefined && stripFrontmatter(doc.data.text).body.trim() !== '')
  const rootNavigate = useNavigate()
  // 66 §3.1: this concept as a reference (latest); `toRef` when the concept loaded, the live title or id otherwise.
  const docRef = (): AttachRef =>
    doc.data
      ? toRef({ kind: 'knowledge', meta: doc.data.meta })
      : { kind: 'knowledge', id, title: live.provider?.meta?.title || id, meta: '' }
  // 59 §5.4: "Ask theuth…" opens a new chat with this document attached (and names a block).
  const ask = (block?: string) => {
    prefillDraft(null, block ? `about block ${block}: ` : '', [docRef()])
    void rootNavigate({ to: '/new' })
  }
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
  // 97b: the knowledge Explorer renders the Library band above both panes; the document only adds its row.
  const frame = {
    backTo: '/knowledge' as const,
    backLabel: 'Back to knowledge',
    title: <code className="min-w-0 flex-1 truncate font-mono text-xs text-fg-tertiary">{id}</code>,
    band: false,
  }

  if (draft !== undefined) {
    return (
      <Frame {...frame}>
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
        {...frame}
        actions={<Action label="Changes" icon={HistoryIcon} onClick={() => setSheetOpen(true)} />}
      >
        <Historical id={id} change={change} snapshot={snapshot} />
        {sheet}
      </Frame>
    )
  }
  if (doc.isPending) {
    return (
      <Frame {...frame}>
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
      <Frame {...frame}>
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
  return (
    <DocumentPage
      {...frame}
      live={live}
      fallback={
        unavailable !== null ? (
          <ReadOnlyDocument concept={concept} reason={unavailable} onMissing={(target, title) => setMissing({ id: target, title })} />
        ) : undefined
      }
      onAsk={ask}
      columnRef={column}
      above={<SearchReturn className="mx-auto mt-2 w-full max-w-[768px] px-4" />}
      rail={
        railOpen ? (
          <>
            <Backlinks ids={concept.cited_by} />
            <Links links={concept.links} onMissing={(target) => setMissing({ id: target, title: target })} />
          </>
        ) : null
      }
      actions={
        <Actions
          concept={concept}
          provider={live.provider}
          railOpen={railOpen}
          onRaw={() => setEditing(true)}
          onHistory={() => setSheetOpen(true)}
          onToggleRail={() => setRailOpen((v) => !v)}
          onAsk={() => ask()}
          onAttach={() => setAttaching(true)}
        />
      }
    >
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
      <ChatPicker open={attaching} onOpenChange={setAttaching} item={docRef()} />
      {createMissing}
      {sheet}
    </DocumentPage>
  )
}

/** Grace before the read-only view replaces a document that is still connecting or still empty. */
const UNAVAILABLE_AFTER_MS = 3000

/**
 * Why the live document cannot be shown (59 §24.y), or `null` to show the editor: the engine refused the binding
 * (`doc_resolve` error — the documents helper missing, `documents.enabled = false`), the stream closed, or
 * `UNAVAILABLE_AFTER_MS` passed since the provider was acquired and the document holds no text — either not live
 * (a link device offline with a cached copy keeps its editor) or live but empty while the file has a body: a
 * binding whose first import failed (the helper was missing when `Ensure` ran) stays bound and empty, which is the
 * blank page of the installed app. Clears on its own once the document goes live with content.
 */
function useUnavailable(live: LiveDocument, fileHasBody: boolean): string | null {
  const [late, setLate] = useState(false)
  const [, setRevision] = useState(0)
  useEffect(() => {
    setLate(false)
    const t = setTimeout(() => setLate(true), UNAVAILABLE_AFTER_MS)
    const doc = live.provider?.doc
    const bump = () => setRevision((n) => n + 1)
    doc?.on('update', bump)
    return () => {
      clearTimeout(t)
      doc?.off('update', bump)
    }
  }, [live.provider])
  if (live.error) return live.error.message
  if (live.status.kind === 'closed') return `closed · ${live.status.reason}`
  if (!late) return null
  const hasText = live.provider !== null && /[^\s]/.test(live.provider.doc.getXmlFragment(FRAGMENT).toString().replace(/<[^>]*>/g, ''))
  if (hasText) return null
  switch (live.status.kind) {
    case 'live':
    case 'readonly':
      return fileHasBody ? 'the document is empty although the file has a body — its import failed (documents helper missing? see the node\'s log)' : null
    case 'offline':
      return 'theuth could not open the document (documents helper missing? see its log)'
    default:
      return 'still connecting'
  }
}

const plugins = { code }
const RESERVED: Record<string, true> = { index: true, log: true }

/**
 * The concept as the pre-59 page rendered it (59 §24.y): the header from `meta`, the markdown body (okf link
 * semantics: external → opener, internal → route, missing → create), on the same paper as the editor, with the
 * reason live editing is unavailable. "Open raw" in the bar still edits the file.
 */
export function ReadOnlyDocument({ concept, reason, onMissing, onSelect }: { concept: Concept; reason?: string; onMissing?: (id: string, title: string) => void; onSelect?: (id: string) => void }) {
  const navigate = useNavigate()
  const meta = concept.meta
  const id = meta.id
  const body = useMemo(() => stripFrontmatter(concept.text).body, [concept.text])
  const exists = useMemo<Record<string, boolean>>(() => Object.fromEntries(concept.links), [concept.links])
  const facts = [meta.scope, meta.source, meta.timestamp].filter((s): s is string => s !== null)
  const components = useMemo<Components>(
    () => ({
      a: ({ href = '', children, node: _node, className, ...rest }) => {
        const target = resolveLink(id, href)
        const kind = classify(href)
        const internal = kind === 'absolute' || kind === 'relative'
        const inert = internal && (target === null || RESERVED[target] === true)
        const missing = target !== null && exists[target] === false
        if (internal && onSelect) {
          return <button type="button" className="text-primary underline disabled:cursor-default disabled:text-fg-tertiary disabled:no-underline" disabled={inert || missing} onClick={() => { if (target) onSelect(target) }}>{children}</button>
        }
        return (
          <a
            href={href}
            {...rest}
            title={inert ? 'Not a document' : missing ? 'Missing: click to create' : undefined}
            className={cn(className, inert && 'cursor-default text-fg-tertiary no-underline', missing && 'text-danger underline decoration-dotted')}
          >
            {children}
          </a>
        )
      },
    }),
    [id, exists, onSelect],
  )
  const onClick = (e: MouseEvent<HTMLDivElement>) => {
    const a = e.target instanceof Element ? e.target.closest('a') : null
    const href = a?.getAttribute('href')
    if (!a || !href) return
    const kind = classify(href)
    if (kind === 'anchor') return
    e.preventDefault()
    if (kind === 'external') {
      openUrl(href).catch((err: unknown) => toast.error(String(err)))
      return
    }
    const target = resolveLink(id, href)
    if (target === null || RESERVED[target]) return
    if (exists[target] === false) onMissing?.(target, a.textContent?.trim() || target)
    else navigate({ to: '/knowledge/$', params: { _splat: target } })
  }
  return (
    <div className="min-h-0 flex-1 overflow-y-auto bg-background md:p-6" data-testid="knowledge-readonly">
      <div className="theuth-paper">
        <header className="mb-6 border-b border-border-subtle pb-4">
          <div className="flex flex-wrap items-center gap-2">
            <h1 className="text-[24px] leading-8 font-semibold">{meta.title}</h1>
            <Badge variant="outline">{meta.type}</Badge>
            <StatusBadge meta={meta} />
            {meta.tags.map((t) => (
              <Badge key={t} variant="secondary">
                {t}
              </Badge>
            ))}
          </div>
          {meta.description && <p className="mt-1 text-fg-secondary">{meta.description}</p>}
          {facts.length > 0 && <p className="mt-1 text-sm text-fg-tertiary">{facts.join(' · ')}</p>}
          {reason && <p className="mt-2 text-xs text-fg-tertiary">live editing unavailable: {reason}</p>}
        </header>
        <div onClickCapture={onClick}>
          <Streamdown mode="static" plugins={plugins} components={components} linkSafety={{ enabled: false }}>
            {body}
          </Streamdown>
        </div>
      </div>
    </div>
  )
}

function Actions({
  concept,
  provider,
  railOpen,
  onRaw,
  onHistory,
  onToggleRail,
  onAsk,
  onAttach,
}: {
  concept: Concept
  provider: EngineProvider | null
  railOpen: boolean
  onRaw: () => void
  onHistory: () => void
  onToggleRail: () => void
  onAsk: () => void
  onAttach: () => void
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
      <Action label="Ask theuth…" icon={Sparkles} onClick={onAsk} />
      <Action label="Attach to chat…" icon={MessageSquarePlus} onClick={onAttach} />
      <UndoAgentAction provider={provider} />
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

/** 59 §5.5 "Undo last agent edit": enabled once the agent has edited this document while it was open. */
export function UndoAgentAction({ provider }: { provider: EngineProvider | null }) {
  const api = useApi()
  const [agentEdits, setAgentEdits] = useState(provider?.agentEdits ?? 0)
  useEffect(() => {
    if (!provider) return
    setAgentEdits(provider.agentEdits)
    return provider.onAuthored((a) => a.by.who === 'agent' && setAgentEdits(provider.agentEdits))
  }, [provider])
  const undo = useMutation({
    mutationFn: () => api.doc_undo_agent(provider!.id),
    onSuccess: () => {
      toast('Undid the last agent edit')
      setAgentEdits(0)
    },
    onError: (e) => toast.error(e.message),
  })
  return (
    <Action
      label="Undo last agent edit"
      icon={Undo2}
      onClick={() => undo.mutate()}
      disabled={!provider || agentEdits === 0 || undo.isPending}
    />
  )
}

/**
 * The complete text of `id` as committed at `snapshot`, read-only (49 §7): never `knowledge_get`, so a
 * deleted concept or a commit outside the newest history page reads the same way. `change` is the
 * commit whose diff the text belongs to: After when it is the snapshot, Before when the snapshot is its parent.
 */
function Historical({ id, change, snapshot }: { id: string; change: string; snapshot: string }) {
  const canQuery2 = useQuerySupport(["knowledge_read_revision"])
  const canQuery2_2 = useQuerySupport(["knowledge_revision"])
  const api = useApi()
  const origin = useSearchOrigin()
  const view = useSearchState((s) => s.view)
  const side = snapshot === change ? 'After' : 'Before'
  const text = useQuery({ enabled: canQuery2,
    queryKey: queryKeys.knowledgeReadRevision(id, snapshot),
    queryFn: () => api.knowledge_read_revision(id, snapshot),
    staleTime: Number.POSITIVE_INFINITY,
  })
  const meta = useQuery({ enabled: canQuery2_2,
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
