import { useQuerySupport } from '@/rpc/capabilities'
// 144 §12.2: `/sources/$id` — a read-only `DocumentPage` with Content · Insights · Details.
// Wave 2 (U1): the way back to the notebook (§4, `useNotebookBack`) and the Insights tab (§4.3): the `## Insights`
// sections of the document, **Run ▾** (`insight_skills` → `insight_run`), **Promote to note** (`insight_promote`).
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Link, getRouteApi, useNavigate } from '@tanstack/react-router'
import { ChevronDown, Loader2 } from 'lucide-react'
import { type ReactNode, useState } from 'react'
import { notify as toast } from '@/lib/notify'
import { type DocBlock, plainText, useBlocks } from '@/components/board/blocks'
import { DocumentPage, useNotebookBack } from '@/components/editor/DocumentPage'
import { useDocument } from '@/components/editor/useDocument'
import { useItemNotebooks, useSource } from '@/components/notebooks/useNotebook'
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
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import type { EngineProvider } from '@/lib/doc/provider'
import { fmtBytes } from '@/lib/files'
import { notebookKeys, useContextApi, useNotebooksApi } from '@/lib/notebooks-rpc'
import type { Source as NotebookSource } from "@/rpc/bindings/method.document.output"
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
  // 144 §4: back to the notebook this source was opened from (or belongs to), at every width; the Sources tab on the phone.
  const wayBack = useNotebookBack('source', id, search.nb)
  return (
    <DocumentPage
      backTo="/notebooks"
      backLabel="Back to notebooks"
      back={wayBack.back}
      live={readOnlyLive}
      title={
        <div className="flex min-w-0 flex-1 items-center gap-2">
          {glyph && <span className={cn('flex size-[18px] shrink-0 items-center justify-center rounded text-[10px] font-bold text-white', glyph.className)}>{glyph.text}</span>}
          <h2 className="min-w-0 truncate text-sm font-semibold">{s ? sourceTitle(s) : src.isError ? err(src.error) : '…'}</h2>
          {wayBack.chip}
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
              <span className="font-mono">0:00 / {mmss(s.duration_ms ?? 0)}</span>
            </div>
          )}
        </>
      }
      fallback={
        tab === 'insights' ? (
          live.provider ? (
            <Insights source={id} provider={live.provider} nb={search.nb} />
          ) : (
            <p className="p-6 text-sm text-fg-tertiary">{live.error ? live.error.message : 'Loading…'}</p>
          )
        ) : tab === 'details' ? (
          s ? (
            <Details source={s} nb={search.nb} />
          ) : (
            <p className="p-6 text-sm text-fg-tertiary">{src.isError ? err(src.error) : 'Loading…'}</p>
          )
        ) : undefined
      }
    />
  )
}

export type InsightSection = { id: string; name: string; model: string | null; date: string | null; blocks: DocBlock[] }

const BUILTINS: Record<string, string> = { key_insights: 'Key insights', toc: 'Table of contents', open_questions: 'Open questions' }

/**
 * Pure (§4.3): the `###` sections under `## Insights` — `id` is the heading block (the first block of the section,
 * what `insight_promote` takes), the heading text is `<name> · <model> · <date>`. Nothing before the first `###`.
 */
export function insightSections(blocks: readonly DocBlock[]): InsightSection[] {
  const out: InsightSection[] = []
  let inside = false
  let cur: InsightSection | null = null
  for (const b of blocks) {
    if (b.type === 'heading' && b.props.level <= 2) {
      inside = b.props.level === 2 && plainText(b.content).trim().toLowerCase() === 'insights'
      cur = null
      continue
    }
    if (!inside) continue
    if (b.type === 'heading' && b.props.level === 3) {
      const [name = '', model = null, date = null] = plainText(b.content).split(' · ').map((p) => p.trim())
      cur = { id: b.id, name, model, date, blocks: [] }
      out.push(cur)
      continue
    }
    cur?.blocks.push(b)
  }
  return out
}

function Insights({ source, provider, nb }: { source: string; provider: EngineProvider; nb: string | undefined }) {
  const canQuery1 = useQuerySupport(["insight_skills"])
  const ctx = useContextApi()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const sections = insightSections(useBlocks(provider))
  const skills = useQuery({ enabled: canQuery1, queryKey: notebookKeys.insightSkills, queryFn: () => ctx.insight_skills() })
  // The new section appears on the next `SourceEvent::Changed` through the document stream; `running` covers the gap.
  const run = useMutation({
    mutationFn: (name: string) => ctx.insight_run(source, name in BUILTINS ? { builtin: name, skill: null } : { builtin: null, skill: name }),
    onSuccess: () => void qc.invalidateQueries({ queryKey: notebookKeys.source(source) }),
    onError: (e) => toast.error(e.message),
  })
  const promote = useMutation({
    mutationFn: (block: string) => ctx.insight_promote(source, block, nb ?? null),
    onSuccess: (doc) =>
      toast('Promoted to a note', { action: { label: 'Open', onClick: () => void navigate({ to: '/notes/$id', params: { id: doc }, search: nb ? { nb } : {} }) } }),
    onError: (e) => toast.error(e.message),
  })
  // `insight_skills()`: the three builtins first, then skills (`notebook_insight: true`); the builtins alone until it answers.
  const names = skills.data ?? Object.keys(BUILTINS)
  const builtins = names.filter((n) => n in BUILTINS)
  const custom = names.filter((n) => !(n in BUILTINS))
  return (
    <div className="mx-auto w-full max-w-3xl overflow-y-auto px-6 py-4 text-sm">
      <div className="flex items-center gap-2">
        <h3 className="text-xs font-semibold text-fg-secondary">Insights · {sections.length}</h3>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button variant="outline" size="sm" className="ml-auto" disabled={run.isPending}>
              {run.isPending ? <Loader2 className="animate-spin" /> : null}
              {run.isPending ? 'Running…' : 'Run'} <ChevronDown />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            {skills.isError && <DropdownMenuItem disabled>{skills.error.message}</DropdownMenuItem>}
            {builtins.map((name) => (
              <DropdownMenuItem key={name} onSelect={() => run.mutate(name)}>
                {BUILTINS[name]}
              </DropdownMenuItem>
            ))}
            {custom.length > 0 && (
              <>
                <DropdownMenuSeparator />
                <DropdownMenuLabel>Skills</DropdownMenuLabel>
                {custom.map((name) => (
                  <DropdownMenuItem key={name} onSelect={() => run.mutate(name)}>
                    {name}
                  </DropdownMenuItem>
                ))}
              </>
            )}
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
      {sections.length === 0 ? (
        <p className="mt-3 text-fg-tertiary">No insights yet {'\u2014'} Run ▾ generates one.</p>
      ) : (
        <ul className="mt-3 flex flex-col gap-4">
          {sections.map((sec) => (
            <li key={sec.id} id={sec.id} className="rounded-lg border border-border-subtle p-3">
              <div className="flex items-baseline gap-2">
                <h4 className="min-w-0 flex-1 truncate font-semibold">{sec.name || 'Insight'}</h4>
                {(sec.model || sec.date) && <span className="shrink-0 text-[11px] text-fg-tertiary">{[sec.model, sec.date].filter(Boolean).join(' · ')}</span>}
                <Button variant="ghost" size="xs" className="shrink-0" disabled={promote.isPending} onClick={() => promote.mutate(sec.id)}>
                  Promote to note
                </Button>
              </div>
              <div className="mt-2 flex flex-col gap-1.5 text-fg-secondary">
                {sec.blocks.map((b) => (
                  <p key={b.id} className={cn(b.type === 'bulletListItem' || b.type === 'numberedListItem' ? 'pl-4 before:absolute before:-ml-3 before:content-["•"]' : undefined, 'relative')}>
                    {plainText(b.content)}
                  </p>
                ))}
              </div>
            </li>
          ))}
        </ul>
      )}
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
  const member = useItemNotebooks('source', id)
  const inNotebooks = member ?? []
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
        <p className="text-xs text-fg-tertiary">{member === undefined ? 'Loading…' : 'None.'}</p>
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
