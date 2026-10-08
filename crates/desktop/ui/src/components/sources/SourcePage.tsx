// 144 §12.2: `/sources/$id` — a read-only `DocumentPage` with Content · Insights · Details.
import { useQueries, useQueryClient } from '@tanstack/react-query'
import { Link, getRouteApi, useNavigate } from '@tanstack/react-router'
import { ChevronDown } from 'lucide-react'
import { type ReactNode, useState } from 'react'
import { toast } from 'sonner'
import { DocumentPage } from '@/components/editor/DocumentPage'
import { useDocument } from '@/components/editor/useDocument'
import { useSource } from '@/components/notebooks/useNotebook'
import { useNotebooks } from '@/components/notebooks/useNotebooks'
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
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { fmtBytes } from '@/lib/files'
import { notebookKeys, useNotebooksApi } from '@/lib/notebooks-rpc'
import type { NotebookSource } from '@/rpc/bindings/NotebookSource'
import { useApi } from '@/lib/rpc'
import { formatAt } from '@/lib/time'
import { cn } from '@/lib/utils'
import { isMedia, originDetail, originGlyph, originLabel, sourceTitle, statusWord } from './origin'

const route = getRouteApi('/sources_/$id')

type Tab = 'content' | 'insights' | 'details'
const TABS: Tab[] = ['content', 'insights', 'details']

const err = (e: unknown) => (e instanceof Error ? e.message : String(e))
const mmss = (ms: number) => `${Math.floor(ms / 60_000)}:${String(Math.floor((ms % 60_000) / 1000)).padStart(2, '0')}`

export function SourcePage() {
  const { id } = route.useParams()
  const search = route.useSearch()
  const tab: Tab = search.tab ?? 'content'
  const navigate = useNavigate()
  const src = useSource(id)
  const live = useDocument({ doc: id })
  // `editable: false` (§12.2): the editor edits only on `live`/`offline`.
  const readOnlyLive = { ...live, status: live.status.kind === 'live' || live.status.kind === 'offline' ? ({ kind: 'readonly' } as const) : live.status }
  const s = src.data
  const glyph = s && originGlyph(s.origin)
  return (
    <DocumentPage
      backTo="/notebooks"
      backLabel="Back to notebooks"
      live={readOnlyLive}
      title={
        <div className="flex min-w-0 flex-1 items-center gap-2">
          {glyph && <span className={cn('flex size-[18px] shrink-0 items-center justify-center rounded text-[10px] font-bold text-white', glyph.className)}>{glyph.text}</span>}
          <h2 className="min-w-0 truncate text-sm font-semibold">{s ? sourceTitle(s) : src.isError ? err(src.error) : '…'}</h2>
          {search.nb && (
            <Link to="/notebooks/$id" params={{ id: search.nb }} search={{}} className="shrink-0 text-xs text-fg-tertiary hover:underline">
              ← notebook
            </Link>
          )}
        </div>
      }
      above={
        <>
          <Tabs value={tab} onValueChange={(t) => void navigate({ to: '/sources/$id', params: { id }, search: { ...search, tab: t as Tab }, replace: true })} className="gap-0">
            <TabsList variant="line" className="w-full justify-start border-b border-border-subtle px-2">
              {TABS.map((t) => (
                <TabsTrigger key={t} value={t} className="flex-none capitalize">
                  {t}
                </TabsTrigger>
              ))}
            </TabsList>
          </Tabs>
          {tab === 'content' && s && isMedia(s) && (
            <div title="wave 2" className="flex items-center gap-3 border-b border-border-subtle px-6 py-2 text-xs text-fg-tertiary">
              <input type="range" disabled className="flex-1" />
              <span className="font-mono">0:00 / {mmss(Number(s.duration_ms ?? 0n))}</span>
            </div>
          )}
        </>
      }
      fallback={tab === 'insights' ? <Insights /> : tab === 'details' ? s ? <Details source={s} nb={search.nb} /> : <p className="p-6 text-sm text-fg-tertiary">{src.isError ? err(src.error) : 'Loading…'}</p> : undefined}
    />
  )
}

function Insights() {
  return (
    <div className="flex flex-col items-start gap-3 p-6 text-sm text-fg-secondary">
      <p>Insights land in wave 2.</p>
      <Button variant="outline" size="sm" disabled title="wave 2">
        Run <ChevronDown />
      </Button>
    </div>
  )
}

function Details({ source, nb }: { source: NotebookSource; nb?: string }) {
  const api = useApi()
  const nbApi = useNotebooksApi()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const id = source.doc.id
  const title = sourceTitle(source)
  const [confirming, setConfirming] = useState(false)
  const notebooks = useNotebooks(false)
  // ponytail: O(notebooks) item lists, fine under ~50 notebooks; a source→notebooks RPC if it grows
  const lists = useQueries({
    queries: (notebooks.data ?? []).map((n) => ({
      queryKey: notebookKeys.items(n.doc.id),
      queryFn: async () => (await nbApi.notebook_items_list(n.doc.id)).filter((i) => i.removed_ms === null),
    })),
  })
  const inNotebooks = (notebooks.data ?? []).filter((_, i) => lists[i]?.data?.some((it) => it.kind === 'source' && it.item === id))
  const others = inNotebooks.filter((n) => n.doc.id !== nb).length
  const status = statusWord(source)

  const refresh = () =>
    nbApi.source_refresh(id).then(
      () => {
        toast('Re-fetching…')
        void qc.invalidateQueries({ queryKey: notebookKeys.source(id) })
        void qc.invalidateQueries({ queryKey: ['notebooks', 'sources'] })
      },
      (e: unknown) => toast.error(err(e)),
    )
  const remove = (notebook: string) =>
    nbApi.notebook_item_remove(notebook, 'source', id).then(
      () => {
        for (const k of [notebookKeys.items(notebook), notebookKeys.sources(notebook), notebookKeys.one(notebook)]) void qc.invalidateQueries({ queryKey: k })
      },
      (e: unknown) => toast.error(err(e)),
    )
  const del = () =>
    api.doc_delete(id).then(
      () => {
        toast(`Deleted “${title}”`)
        void qc.invalidateQueries({ queryKey: notebookKeys.all })
        void (nb ? navigate({ to: '/notebooks/$id', params: { id: nb }, search: {} }) : navigate({ to: '/notebooks', search: {} }))
      },
      (e: unknown) => toast.error(err(e)),
    )

  const size = [
    source.bytes !== null && fmtBytes(Number(source.bytes)),
    source.words !== null && `${source.words} words`,
    source.pages !== null && `${source.pages} pages`,
    source.duration_ms !== null && mmss(Number(source.duration_ms)),
  ].filter((x): x is string => typeof x === 'string')
  const original = source.origin.kind === 'url' ? (source.origin.canonical ?? source.origin.url) : source.origin.kind === 'youtube' ? source.origin.url : null

  return (
    <div className="mx-auto w-full max-w-3xl overflow-y-auto px-6 py-4 text-sm">
      <dl className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1.5">
        <Row k="origin">
          {originLabel(source.origin)} · <span className="wrap-anywhere">{originDetail(source.origin)}</span>
        </Row>
        <Row k="status">
          {status.text.replace(' · retry', '')}
          {source.attempts > 0 && ` · ${source.attempts} attempt${source.attempts === 1 ? '' : 's'}`}
          {source.error && <span className="text-danger"> · {source.error}</span>}
        </Row>
        <Row k="fetched">{source.fetched_ms !== null ? formatAt(Number(source.fetched_ms)) : '—'}</Row>
        <Row k="content sha">{source.content_sha ? <span className="font-mono">{source.content_sha.slice(0, 12)}</span> : '—'}</Row>
        <Row k="changed since">{source.changed_since !== null ? formatAt(Number(source.changed_since)) : '—'}</Row>
        <Row k="size">{size.length ? size.join(' · ') : '—'}</Row>
      </dl>
      <div className="mt-4 flex flex-wrap gap-2">
        <Button variant="outline" size="sm" onClick={() => void refresh()}>
          Re-fetch
        </Button>
        {original ? (
          <Button variant="outline" size="sm" asChild>
            <a href={original} target="_blank" rel="noreferrer">
              Open original
            </a>
          </Button>
        ) : (
          <Button variant="outline" size="sm" disabled title="hub-local">
            Open original
          </Button>
        )}
      </div>
      <h3 className="mt-6 mb-1.5 text-xs font-semibold text-fg-secondary">In notebooks</h3>
      {inNotebooks.length === 0 ? (
        <p className="text-xs text-fg-tertiary">{notebooks.isPending || lists.some((l) => l.isPending) ? 'Loading…' : 'None.'}</p>
      ) : (
        <ul className="flex flex-col gap-1">
          {inNotebooks.map((n) => (
            <li key={n.doc.id} className="flex items-center gap-2">
              <Link to="/notebooks/$id" params={{ id: n.doc.id }} search={{}} className="min-w-0 truncate hover:underline">
                {n.doc.title || 'Untitled'}
              </Link>
              <Button variant="ghost" size="xs" className="text-fg-tertiary" onClick={() => void remove(n.doc.id)}>
                Remove
              </Button>
            </li>
          ))}
        </ul>
      )}
      <div className="mt-6">
        <Button variant="outline" size="sm" className="text-danger" onClick={() => setConfirming(true)}>
          Delete
        </Button>
      </div>
      <AlertDialog open={confirming} onOpenChange={setConfirming}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete “{title}”?</AlertDialogTitle>
            <AlertDialogDescription>
              The source and its text are removed for good.{others > 0 && ` Also in ${others} other notebook${others === 1 ? '' : 's'}.`}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction onClick={() => void del()}>Delete</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}

function Row({ k, children }: { k: string; children: ReactNode }) {
  return (
    <>
      <dt className="text-fg-tertiary">{k}</dt>
      <dd className="min-w-0">{children}</dd>
    </>
  )
}
