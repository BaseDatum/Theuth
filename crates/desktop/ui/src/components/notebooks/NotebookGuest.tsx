import { QueryClient, QueryClientProvider, useQueries, useQuery } from '@tanstack/react-query'
import { ArrowLeft } from 'lucide-react'
import { type ReactNode, useEffect, useMemo, useReducer, useRef, useState, useSyncExternalStore } from 'react'
import { type DocBlock, type Inline, plainText, useBlocks } from '@/components/board/blocks'
import { Editor as BlockEditor } from '@/components/editor/Editor'
import { SourceCard } from '@/components/sources/SourceCard'
import { Button } from '@/components/ui/button'
import { useIsMobile } from '@/hooks/use-mobile'
import { EngineProvider, type GuestUser } from '@/lib/doc/provider'
import { notebookKeys, notebooksApi } from '@/lib/notebooks-rpc'
import type { NotebookItem } from '@/rpc/bindings/NotebookItem'
import { cn } from '@/lib/utils'
import { type Api, makeApi } from '@/rpc/api'
import type { Role } from '@/rpc/bindings/Role'
import type { RpcClient } from '@/rpc/client'

type Open = { kind: 'note' | 'source'; id: string } | null
type GuestTab = 'sources' | 'answers' | 'notes'
const TABS: { key: GuestTab; label: (sources: number, notes: number) => string }[] = [
  { key: 'sources', label: (s) => `Sources · ${s}` },
  { key: 'answers', label: () => 'Answers' },
  { key: 'notes', label: (_, n) => `Notes · ${n}` },
]

/** Pure (144 §8): a saved answer carries `*Answer from [chat](theuth://sessions/…)*`; its other `theuth://` links are the citations kept. */
export function promoted(blocks: readonly DocBlock[]): { promoted: boolean; citations: number } {
  let isPromoted = false
  let citations = 0
  for (const b of blocks) {
    if (!Array.isArray(b.content)) continue
    const hrefs = (b.content as Inline[]).flatMap((c) => (c.type === 'link' && c.href.startsWith('theuth://') ? [c.href] : []))
    if (!isPromoted && hrefs.some((h) => h.startsWith('theuth://sessions/')) && plainText(b.content).includes('Answer from')) isPromoted = true
    citations += hrefs.filter((h) => !h.startsWith('theuth://sessions/')).length
  }
  return { promoted: isPromoted, citations }
}

type Props = { provider: EngineProvider; client: RpcClient; engine: string; user: GuestUser; role: Role }

/**
 * 144 §12.5: the `/d` guest's notebook — Sources (read-only), "Answers the owner promoted", Notes (live, per role).
 * No composer, Add, dial, Overviews or Ask (D13). A note or source opens in place over the columns. The guest
 * container has no query client of its own (`GuestApp` mounts no engine scope), so this page brings one.
 */
export function NotebookGuest(props: Props) {
  const [qc] = useState(() => new QueryClient())
  return (
    <QueryClientProvider client={qc}>
      <Guest {...props} />
    </QueryClientProvider>
  )
}

function Guest({ provider, client, engine, user, role }: Props) {
  const api = useMemo(() => makeApi(client), [client])
  const nb = useMemo(() => notebooksApi(client), [client])
  const id = provider.id
  const items = useQuery({ queryKey: notebookKeys.items(id), queryFn: async () => (await nb.notebook_items_list(id)).filter((i) => i.removed_ms === null) })
  const sourceItems = useMemo(() => items.data?.filter((i) => i.kind === 'source') ?? [], [items.data])
  const noteItems = useMemo(() => (items.data?.filter((i) => i.kind === 'note') ?? []).sort((a, b) => Number(b.added_ms - a.added_ms)), [items.data])
  const sources = useQueries({ queries: sourceItems.map((i) => ({ queryKey: notebookKeys.source(i.item), queryFn: () => nb.source_get(i.item) })) })

  // ponytail: one doc_open per note for its title; a titled items RPC when notebooks grow.
  const notes = useRef<Record<string, EngineProvider>>({})
  const [, bump] = useReducer((n: number) => n + 1, 0)
  const noteIds = noteItems.map((i) => i.item).join('\n')
  useEffect(() => {
    const ids = noteIds ? noteIds.split('\n') : []
    const cur = notes.current
    for (const nid of ids) cur[nid] ??= new EngineProvider(client, api, nid, engine, user)
    for (const nid of Object.keys(cur)) {
      if (ids.includes(nid)) continue
      cur[nid]?.destroy()
      delete cur[nid]
    }
    bump()
  }, [noteIds, client, api, engine, user])
  useEffect(
    () => () => {
      for (const p of Object.values(notes.current)) p.destroy()
      notes.current = {}
    },
    [],
  )

  const [open, setOpen] = useState<Open>(null)
  // A source opens read-only whatever the role: a fresh viewer provider for it, gone on back.
  const [sourceProvider, setSourceProvider] = useState<EngineProvider | null>(null)
  useEffect(() => {
    if (open?.kind !== 'source') return
    const p = new EngineProvider(client, api, open.id, engine, { ...user, role: 'viewer' })
    setSourceProvider(p)
    return () => {
      setSourceProvider(null)
      p.destroy()
    }
  }, [open, client, api, engine, user])

  const mobile = useIsMobile()
  const [tab, setTab] = useState<GuestTab>('notes')
  const openProvider = open?.kind === 'note' ? notes.current[open.id] : open?.kind === 'source' ? sourceProvider : undefined

  const header = (
    <header className="flex h-10 shrink-0 items-center gap-3 border-b border-border-subtle px-4 text-xs">
      {open && (
        <Button variant="ghost" size="sm" onClick={() => setOpen(null)}>
          <ArrowLeft /> back
        </Button>
      )}
      <span className="ml-auto whitespace-nowrap rounded-full border border-border-subtle px-2 py-px text-[11px]">you are a guest · {role}</span>
    </header>
  )

  if (open) {
    return (
      <div className="flex min-h-0 flex-1 flex-col">
        {header}
        {openProvider ? <Opened provider={openProvider} api={api} /> : <div className="flex-1" />}
      </div>
    )
  }

  const sourcesCol = (
    <Col head={`Sources · ${sourceItems.length}`}>
      {sources.map((q, i) => {
        const item = sourceItems[i]
        return q.data && item ? <SourceCard key={item.item} source={q.data} readOnly onOpen={() => setOpen({ kind: 'source', id: item.item })} /> : null
      })}
      {items.isSuccess && sourceItems.length === 0 && <p className="p-2 text-xs text-fg-tertiary">No sources.</p>}
    </Col>
  )
  const answersCol = (
    <Col head="Answers the owner promoted">
      {noteItems.map((i) => {
        const p = notes.current[i.item]
        return p ? <NoteRow key={i.item} item={i} provider={p} role={role} onlyPromoted onOpen={() => setOpen({ kind: 'note', id: i.item })} /> : null
      })}
      <p className="px-1 py-1.5 text-xs text-fg-tertiary">No composer here {'\u2014'} ask the owner, or edit the notes.</p>
    </Col>
  )
  const notesCol = (
    <Col head={`Notes · ${noteItems.length}`}>
      {noteItems.map((i) => {
        const p = notes.current[i.item]
        return p ? <NoteRow key={i.item} item={i} provider={p} role={role} onOpen={() => setOpen({ kind: 'note', id: i.item })} /> : null
      })}
      {items.isSuccess && noteItems.length === 0 && <p className="p-2 text-xs text-fg-tertiary">No notes yet.</p>}
    </Col>
  )

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {header}
      {items.isError && <p className="p-3 text-sm text-danger">{items.error.message}</p>}
      {mobile ? (
        <>
          <div role="tablist" className="grid shrink-0 grid-cols-3 border-b border-border-subtle text-xs">
            {TABS.map((t) => (
              <button
                key={t.key}
                type="button"
                role="tab"
                aria-selected={tab === t.key}
                className={cn('py-2.5 text-center text-fg-tertiary', tab === t.key && 'font-semibold text-fg shadow-[inset_0_-2px_0_var(--color-primary)]')}
                onClick={() => setTab(t.key)}
              >
                {t.label(sourceItems.length, noteItems.length)}
              </button>
            ))}
          </div>
          {tab === 'sources' && sourcesCol}
          {tab === 'answers' && answersCol}
          {tab === 'notes' && notesCol}
        </>
      ) : (
        <div className="grid min-h-0 flex-1 grid-cols-3">
          {sourcesCol}
          <div className="flex min-h-0 min-w-0 flex-col border-l border-border-subtle">{answersCol}</div>
          <div className="flex min-h-0 min-w-0 flex-col border-l border-border-subtle">{notesCol}</div>
        </div>
      )}
    </div>
  )
}

function Col({ head, children }: { head: string; children: ReactNode }) {
  return (
    <section className="flex min-h-0 min-w-0 flex-1 flex-col">
      <header className="flex h-10 shrink-0 items-center gap-2 border-b border-border-subtle px-3 text-xs font-semibold">{head}</header>
      <div className="flex min-h-0 flex-1 flex-col gap-1.5 overflow-y-auto p-2">{children}</div>
    </section>
  )
}

/** An opened note or source; note cells show the replicated latest run through the guest's `api` (171 §13). */
function Opened({ provider, api }: { provider: EngineProvider; api: Api }) {
  const status = useSyncExternalStore((cb) => provider.onStatus(cb), () => provider.status)
  return <BlockEditor provider={provider} status={status} guest={api} />
}

/** One note as the guest sees it: the live title, `live · you may edit` / `live · view only`; `onlyPromoted` = the centre list's rows. */
function NoteRow({ item, provider, role, onlyPromoted = false, onOpen }: { item: NotebookItem; provider: EngineProvider; role: Role; onlyPromoted?: boolean; onOpen: () => void }) {
  const status = useSyncExternalStore((cb) => provider.onStatus(cb), () => provider.status)
  const blocks = useBlocks(provider)
  const info = promoted(blocks)
  if (onlyPromoted && !info.promoted) return null
  const title = provider.meta?.title || 'Untitled'
  const live = status.kind === 'live' || status.kind === 'readonly'
  return (
    <button type="button" onClick={onOpen} className="flex items-center gap-2.5 rounded-lg border border-border-subtle bg-background px-2.5 py-2 text-left hover:bg-layer-2" data-item={item.item}>
      <span aria-hidden className="flex h-[30px] w-[22px] shrink-0 flex-col gap-[3px] rounded-sm border border-border-subtle bg-background px-1 pt-[5px]">
        <span className="h-0.5 bg-border-subtle" />
        <span className="h-0.5 bg-border-subtle" />
        <span className="h-0.5 bg-border-subtle" />
      </span>
      <span className="flex min-w-0 flex-1 flex-col">
        <span className="truncate text-[12.5px] font-semibold">{title}</span>
        <span className="flex items-center gap-1 text-[11px] text-fg-tertiary">
          {onlyPromoted ? (
            <>
              <span className="rounded-full border border-emerald-700/50 px-1.5 text-emerald-700">AI</span> {info.citations} citations
            </>
          ) : (
            <>
              {live ? 'live' : status.kind} · {role === 'editor' ? 'you may edit' : 'view only'}
            </>
          )}
        </span>
      </span>
    </button>
  )
}
