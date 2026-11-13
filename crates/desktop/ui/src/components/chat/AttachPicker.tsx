import { useQueries, useQuery } from '@tanstack/react-query'
import { Command as CommandPrimitive } from 'cmdk'
import { Check, type LucideIcon, Search } from 'lucide-react'
import { type KeyboardEvent, type PointerEvent, type ReactNode, type RefObject, useEffect, useMemo, useRef, useState } from 'react'
import { GLYPH } from '@/components/chat/ArtifactCard'
import { Img } from '@/components/chat/Attachments'
import { KIND_ICON, useOpenRef } from '@/components/chat/AttachChip'
import { useNotes } from '@/components/notes/useNotes'
import { Archived, Notice, Notices, Section, status } from '@/components/search/parts'
import { Button } from '@/components/ui/button'
import { Command, CommandGroup, CommandItem, CommandList } from '@/components/ui/command'
import { Popover, PopoverAnchor, PopoverContent } from '@/components/ui/popover'
import { Sheet, SheetContent, SheetDescription, SheetTitle } from '@/components/ui/sheet'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { useIsMobile } from '@/hooks/use-mobile'
import { type ArtifactRecord, type AttachRef, KIND_LABEL, type Kind, type Source, key, toRef } from '@/lib/attach'
import {
  TABS,
  TAB_LABEL,
  type KeyAction,
  type Tab,
  cap,
  categoriesFor,
  inThisChat,
  keyAction,
  latestVersions,
  loadRecents,
  nextTab,
} from '@/lib/attach-picker'
import { useKeyboardInset } from '@/lib/mobile'
import { isPrivate } from '@/lib/private'
import { queryKeys, useApi } from '@/lib/rpc'
import { LIMITS } from '@/lib/search'
import { SearchQueue } from '@/lib/search-queue'
import { cn } from '@/lib/utils'
import { useEngines } from '@/link/engines'
import type { SearchCategory } from '@/rpc/bindings/method.rpc_2dcompose.input'
import type { SearchGroup } from "@/rpc/bindings/method.rpc_2dcompose.output"
import { useEntries } from '@/store/sessions'

const INDEXING_POLL_MS = 500
const LONG_PRESS_MS = 450
/** Show more steps 10 → 30 → 50; the cap stays put. */
const NEXT_LIMIT: Record<number, number> = { 10: 30, 30: 50 }

/** One activatable row: cmdk value/React key is the reference identity, never an index. */
type Row = { key: string; ref: AttachRef; Icon: LucideIcon; badges: string[]; archived?: boolean; artifact?: ArtifactRecord }

function row(src: Source, now: Date, extra: { badges?: string[]; archived?: boolean } = {}): Row {
  const ref = toRef(src, now)
  const artifact = src.kind === 'artifact' ? src.artifact : undefined
  return {
    key: key(ref),
    ref,
    Icon: artifact ? GLYPH[artifact.kind] : KIND_ICON[ref.kind],
    badges: artifact ? [`v${artifact.version}`, ...(extra.badges ?? [])] : (extra.badges ?? []),
    archived: extra.archived,
    artifact,
  }
}

const fromRef = (ref: AttachRef): Row => ({ key: key(ref), ref, Icon: KIND_ICON[ref.kind], badges: [] })

type Group = { id: string; heading: string; rows: Row[]; footer?: ReactNode; section?: ReactNode }

const GROUP_ORDER: readonly Kind[] = ['note', 'knowledge', 'artifact', 'session', 'meeting']

/** `(min-width: 960px)`: room for the preview pane (§7.1). */
function useWide(): boolean {
  const [wide, setWide] = useState(() => typeof matchMedia === 'function' && matchMedia('(min-width: 960px)').matches)
  useEffect(() => {
    const mq = matchMedia('(min-width: 960px)')
    const on = () => setWide(mq.matches)
    mq.addEventListener('change', on)
    return () => mq.removeEventListener('change', on)
  }, [])
  return wide
}

/**
 * The one picker (66 §7): cmdk in a radix `Popover` anchored to the composer on desktop, a bottom `Sheet` on phones.
 * Empty query → Recent, In this chat and the browse lists; a query → the `search` RPC for chats/artifacts/knowledge/
 * history plus a client-side filter over the notes and meetings lists (phase 5 swaps that for search categories).
 * `at` (the `@` trigger, §7.8) is quiet mode: the composer's textarea keeps focus and supplies the query; this
 * component intercepts ↑ ↓ ⏎ ⇥ Esc at the window and leaves every other key to the textarea.
 */
export function AttachPicker({
  session,
  open,
  at = null,
  anchor,
  onClose,
  onAttach,
  onEmpty,
}: {
  session: string | null
  open: boolean
  /** Quiet `@` mode: the word after `@`; `null` when opened from `+`. */
  at?: string | null
  anchor: RefObject<HTMLElement | null>
  onClose(): void
  /** Attach these and close. */
  onAttach(refs: AttachRef[]): void
  /** Quiet mode: the query matched nothing anywhere, the composer leaves the text as typed. */
  onEmpty?(): void
}) {
  const api = useApi()
  const engine = useEngines((s) => s.active)
  const isMobile = useIsMobile()
  const wide = useWide()
  const openRef = useOpenRef()
  const inset = useKeyboardInset()
  const quiet = at !== null
  const [tab, setTab] = useState<Tab>('all')
  const [typed, setTyped] = useState('')
  const [debounced, setDebounced] = useState('')
  const [selected, setSelected] = useState<Map<string, AttachRef>>(new Map())
  const [highlighted, setHighlighted] = useState('')
  const [preview, setPreview] = useState(true)
  const [showArchived, setShowArchived] = useState(false)
  const [limits, setLimits] = useState<Partial<Record<SearchCategory, number>>>({})
  const [pressed, setPressed] = useState<Row | null>(null)
  const inputRef = useRef<HTMLInputElement>(null)
  const pressTimer = useRef<number | null>(null)
  const now = useMemo(() => new Date(), [open]) // eslint-disable-line react-hooks/exhaustive-deps -- one clock per open
  /** This surface's request discipline (lib/search-queue): resetting debounce, one `search` in flight, latest settled query pending. */
  const queue = useRef(new SearchQueue()).current

  const query = (quiet ? at : typed).trim()
  useEffect(() => {
    if (!open) {
      queue.clear()
      setTab('all')
      setTyped('')
      setDebounced('')
      setSelected(new Map())
      setHighlighted('')
      setLimits({})
      setPressed(null)
      return
    }
  }, [open, queue])
  useEffect(() => {
    if (query === debounced) return
    const settle = (q: string) => {
      setDebounced(q)
      setLimits({})
    }
    // An emptied box settles at once: the pending query is dropped and browsing returns without a debounce.
    if (query === '') {
      queue.clear()
      settle('')
      return
    }
    queue.type(query, settle)
    return () => queue.cancel()
  }, [query, debounced, queue])

  // ---- Browse sources (§7.4), fetched in parallel, shown as they arrive.
  const browsing = open && debounced === ''
  const notes = useNotes()
  const knowledge = useQuery({
    queryKey: [...queryKeys.knowledgeList, 'attach'],
    queryFn: () => api.knowledge_list(null, null, null, 30),
    enabled: open && (tab === 'all' || tab === 'knowledge') && browsing,
  })
  const artifacts = useQuery({
    queryKey: [...queryKeys.artifacts, 'all'],
    queryFn: () => api.list_artifacts(null, true),
    enabled: open && (tab === 'all' || tab === 'artifact') && browsing,
  })
  const mine = useQuery({
    queryKey: [...queryKeys.artifacts, 'session', session],
    queryFn: () => api.list_artifacts(session, false),
    enabled: open && session !== null && tab === 'all' && browsing,
  })
  const sessions = useQuery({ queryKey: queryKeys.sessions, queryFn: () => api.list_sessions(), enabled: open })
  const meetings = useQuery({
    queryKey: queryKeys.meetings,
    queryFn: () => api.meeting_list(30),
    enabled: open && (tab === 'all' || tab === 'meeting') && browsing,
  })
  const entries = useEntries(session ?? '')
  const recents = useMemo(() => (open ? loadRecents(engine) : []), [open, engine])

  // ---- Query path: one `search` per tab selection, per-category Show more pages, a poll while a group indexes (49 §4.5).
  const cats = categoriesFor(tab)
  const searching = open && debounced !== '' && cats.length > 0
  const base = useQuery({
    queryKey: queryKeys.search(debounced, cats, LIMITS[0], -1),
    queryFn: () => queue.run(debounced, () => api.search(debounced, cats, LIMITS[0])),
    enabled: searching,
    refetchInterval: (q) => (q.state.data?.groups.some((g) => g.page.state === 'indexing') ? INDEXING_POLL_MS : false),
  })
  const moreCats = cats.filter((c) => (limits[c] ?? LIMITS[0]) > LIMITS[0])
  const more = useQueries({
    queries: moreCats.map((c) => ({
      queryKey: queryKeys.search(debounced, [c], limits[c] ?? LIMITS[0], -1),
      queryFn: () => queue.run(debounced, () => api.search(debounced, [c], limits[c] ?? LIMITS[0])),
      enabled: searching,
    })),
  })
  const sessionTitle = (id: string) => sessions.data?.find((s) => s.id === id)?.title
  const project = session ? sessions.data?.find((s) => s.id === session)?.project : undefined

  const groups: Group[] = []
  const searchGroup = (c: SearchCategory) => {
    const m = more[moreCats.indexOf(c)]
    const g: SearchGroup | undefined = m?.data?.groups[0] ?? base.data?.groups.find((x) => x.category === c)
    const kind: Kind =
      c === 'sessions' ? 'session' : c === 'artifacts' ? 'artifact' : c === 'notes' ? 'note' : c === 'meetings' ? 'meeting' : 'knowledge'
    const heading = c === 'knowledge_history' ? 'Knowledge history' : TAB_LABEL[kind]
    if (!g) {
      if (base.error)
        groups.push({
          id: c,
          heading,
          rows: [],
          section: (
            <Section category={c} label={heading} state="error">
              <Notice tone="error">
                {base.error instanceof Error ? base.error.message : String(base.error)}
                <Button variant="outline" size="sm" className="ml-2" onClick={() => void base.refetch()}>
                  Retry
                </Button>
              </Notice>
            </Section>
          ),
        })
      return
    }
    if (g.page.state !== 'ready') {
      groups.push({
        id: c,
        heading,
        rows: [],
        section: (
          <Section category={c} label={heading} state={status(g.page)}>
            <Notices notices={g.page.notices} />
            {g.page.state === 'error' && (
              <div className="px-2 pb-2">
                <Button variant="outline" size="sm" onClick={() => void api.search_refresh([c]).then(() => base.refetch())}>
                  Retry
                </Button>
              </div>
            )}
          </Section>
        ),
      })
      return
    }
    let rows: Row[]
    switch (g.category) {
      case 'sessions':
        rows = g.page.hits.filter((h) => h.session !== session).map((h) => row({ kind: 'session', hit: h }, now, { archived: h.archived }))
        break
      case 'artifacts':
        rows = g.page.hits.map((h) => row({ kind: 'artifact', artifact: h.artifact, sessionTitle: sessionTitle(h.artifact.session) }, now))
        break
      case 'knowledge':
        rows = g.page.hits.map((h) => row({ kind: 'knowledge', meta: h.meta }, now))
        break
      case 'knowledge_history':
        rows = g.page.hits.map((h) => row({ kind: 'knowledge', revision: h }, now, { badges: ['revision'] }))
        break
      case 'notes':
        rows = g.page.hits.map((h) => row({ kind: 'note', doc: h.doc }, now))
        break
      case 'meetings':
        rows = g.page.hits.map((h) => row({ kind: 'meeting', meeting: h.meeting }, now))
        break
      case 'sources':
        rows = g.page.hits.map((h) => row({ kind: 'source', hit: h }, now))
        break
      case 'notebooks':
        rows = g.page.hits.map((h) => row({ kind: 'notebook', hit: h }, now))
        break
    }
    const limit = limits[c] ?? LIMITS[0]
    const loadingMore = !!m && !m.data && !m.error
    groups.push({
      id: c,
      heading: `${heading} · ${status(g.page)}${g.page.coverage === 'partial' ? ' · some content not searched' : ''}`,
      rows,
      footer: g.page.has_more ? (
        <div className="px-2 pt-1 pb-2 text-xs text-fg-tertiary">
          {!(limit in NEXT_LIMIT) ? (
            'More matches; refine your query.'
          ) : (
            <Button
              variant="outline"
              size="sm"
              className="pointer-coarse:min-h-11"
              disabled={loadingMore}
              onClick={() => setLimits((l) => ({ ...l, [c]: NEXT_LIMIT[limit] ?? limit }))}
            >
              {loadingMore ? 'Loading more…' : 'Show more'}
            </Button>
          )}
        </div>
      ) : undefined,
    })
  }

  if (browsing) {
    if (tab === 'all') {
      const recent = recents.slice(0, 8).map(fromRef)
      if (recent.length) groups.push({ id: 'recent', heading: 'Recent', rows: recent })
      if (session) {
        const rows = inThisChat(mine.data ?? [], entries, 6, now).map(fromRef)
        if (rows.length) groups.push({ id: 'chat', heading: 'In this chat', rows })
      }
    }
    for (const kind of GROUP_ORDER) {
      if (tab !== 'all' && tab !== kind) continue
      const n = cap(tab, kind)
      switch (kind) {
        case 'note':
          groups.push({
            id: 'note',
            heading: TAB_LABEL.note,
            rows: [...(notes.data ?? [])]
              .sort((a, b) => Number(b.updated_ms) - Number(a.updated_ms))
              .slice(0, n)
              .map((d) => row({ kind: 'note', doc: d }, now)),
          })
          break
        case 'knowledge':
          groups.push({
            id: 'knowledge',
            heading: TAB_LABEL.knowledge,
            rows: [...(knowledge.data ?? [])]
              .sort((a, b) => (b.timestamp ?? '').localeCompare(a.timestamp ?? ''))
              .slice(0, n)
              .map((m) => row({ kind: 'knowledge', meta: m }, now)),
          })
          break
        case 'artifact':
          groups.push({
            id: 'artifact',
            heading: TAB_LABEL.artifact,
            rows: latestVersions(artifacts.data ?? [])
              .slice(0, n)
              .map((a) => {
                const p = sessions.data?.find((s) => s.id === a.session)?.project
                return row({ kind: 'artifact', artifact: a, sessionTitle: sessionTitle(a.session) }, now, {
                  badges: p && p !== project ? [p] : [],
                })
              }),
          })
          break
        case 'session':
          groups.push({
            id: 'session',
            heading: TAB_LABEL.session,
            rows: (sessions.data ?? [])
              .filter(
                (s) =>
                  s.parent === null &&
                  s.id !== session &&
                  (showArchived || !s.archived) &&
                  (s.retention !== 'private' || (session !== null && isPrivate(session))),
              )
              .slice(0, n)
              .map((s) => row({ kind: 'session', session: s }, now, { badges: s.project ? [s.project] : [], archived: s.archived })),
            footer:
              tab === 'session' ? (
                <label className="flex items-center gap-2 px-2 pb-2 text-xs text-fg-tertiary">
                  <input type="checkbox" checked={showArchived} onChange={(e) => setShowArchived(e.target.checked)} /> Show archived
                </label>
              ) : undefined,
          })
          break
        case 'meeting':
          groups.push({
            id: 'meeting',
            heading: TAB_LABEL.meeting,
            rows: (meetings.data ?? []).slice(0, n).map((m) => row({ kind: 'meeting', meeting: m }, now)),
          })
          break
      }
    }
  } else if (open) {
    for (const c of cats) searchGroup(c)
  }
  const shown = groups.filter((g) => g.rows.length > 0 || g.section || (browsing && tab !== 'all'))
  const flat = shown.flatMap((g) => g.rows)
  const settled = !searching || (!!base.data && more.every((m) => m.data || m.error))
  const pending = query !== debounced
  const hits = flat.length

  // Quiet mode: nothing matched anywhere once the answer is in → the composer keeps the text as typed.
  useEffect(() => {
    if (open && quiet && query !== '' && settled && !pending && hits === 0 && !base.isFetching) onEmpty?.()
  }, [open, quiet, query, settled, pending, hits, base.isFetching, onEmpty])

  // Keep the highlight on a row that exists.
  const current = flat.find((r) => r.key === highlighted) ?? flat[0]
  useEffect(() => {
    if (current && current.key !== highlighted) setHighlighted(current.key)
  }, [current, highlighted])

  const toggle = (r: Row) =>
    setSelected((s) => {
      const n = new Map(s)
      if (n.has(r.key)) n.delete(r.key)
      else n.set(r.key, r.ref)
      return n
    })
  const attach = (r?: Row) => {
    const refs = [...selected.values()]
    if (r && !selected.has(r.key)) refs.push(r.ref)
    if (refs.length > 0) onAttach(refs)
  }
  const act = (a: KeyAction | null): boolean => {
    if (!a) return false
    switch (a.type) {
      case 'close':
        onClose()
        return true
      case 'attach':
        if (pending && !quiet) {
          setDebounced(query)
          return true
        }
        attach(current)
        return true
      case 'toggle':
        if (current) toggle(current)
        return true
      case 'tab':
        setTab((t) => nextTab(t, a.dir))
        return true
      case 'jump':
        setTab(TABS[a.index] ?? 'all')
        return true
      case 'preview':
        setPreview(a.open)
        return true
      case 'back':
        setTab('all')
        return true
    }
  }
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.nativeEvent.isComposing) return
    if (act(keyAction(e, query === ''))) {
      e.preventDefault()
      e.stopPropagation()
    }
  }
  // Quiet mode: the textarea has focus; intercept the navigation keys before React's root sees them.
  const move = (dir: 1 | -1) => {
    const i = flat.findIndex((r) => r.key === current?.key)
    const next = flat[(i + dir + flat.length) % flat.length]
    if (next) setHighlighted(next.key)
  }
  const quietRef = useRef({ move, act })
  quietRef.current = { move, act }
  useEffect(() => {
    if (!open || !quiet) return
    const on = (e: globalThis.KeyboardEvent) => {
      if (e.isComposing) return
      const { move, act } = quietRef.current
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') move(e.key === 'ArrowDown' ? 1 : -1)
      else if (e.key === 'Enter' || e.key === 'Tab') {
        if (e.key === 'Enter' && e.shiftKey) return
        act({ type: 'attach' })
      } else if (e.key === 'Escape') act({ type: 'close' })
      else return
      e.preventDefault()
      e.stopPropagation()
    }
    window.addEventListener('keydown', on, true)
    return () => window.removeEventListener('keydown', on, true)
  }, [open, quiet])

  // Long press (phone): a preview sheet with Open and Attach.
  const press = {
    onPointerDown: (r: Row) => (e: PointerEvent) => {
      if (e.pointerType === 'mouse') return
      pressTimer.current = window.setTimeout(() => setPressed(r), LONG_PRESS_MS)
    },
    cancel: () => {
      if (pressTimer.current) clearTimeout(pressTimer.current)
      pressTimer.current = null
    },
  }

  const announce = settled && searching ? `${hits} results in ${shown.filter((g) => g.rows.length > 0).length} groups` : ''

  const tabs = (
    <ToggleGroup
      type="single"
      value={tab}
      onValueChange={(v) => v && setTab(v as Tab)}
      aria-label="Attach from"
      spacing={1}
      className="w-full justify-start overflow-x-auto px-2 py-1.5 [scrollbar-width:none]"
      onKeyDown={(e) => e.stopPropagation()}
    >
      {TABS.map((t) => (
        <ToggleGroupItem
          key={t}
          value={t}
          className="h-7 shrink-0 rounded-full border border-transparent px-3 text-xs text-fg-secondary data-[state=on]:border-fg-accent data-[state=on]:bg-accent data-[state=on]:text-foreground pointer-coarse:min-h-9"
        >
          {TAB_LABEL[t]}
        </ToggleGroupItem>
      ))}
    </ToggleGroup>
  )

  const list = (
    <CommandList className={cn('min-h-0 flex-1 overscroll-contain', isMobile ? 'max-h-none' : 'max-h-none')} aria-busy={searching && base.isFetching}>
      {shown.length === 0 && (
        <Notice>
          {searching && !settled
            ? 'Searching…'
            : query !== ''
              ? 'No matches.'
              : browsing && (notes.isPending || sessions.isPending)
                ? 'Loading…'
                : 'Nothing to attach yet.'}
        </Notice>
      )}
      {shown.map((g) =>
        g.section ? (
          <div key={g.id}>{g.section}</div>
        ) : (
          <CommandGroup key={g.id} heading={g.heading} className={cn('pt-1', pending && searching && 'opacity-50')}>
            {g.rows.length === 0 && <div className="px-2 py-2 text-xs text-fg-tertiary">Nothing here yet.</div>}
            {g.rows.map((r) => {
              const on = selected.has(r.key)
              return (
                <CommandItem
                  key={r.key}
                  value={r.key}
                  onSelect={() => (isMobile ? toggle(r) : attach(r))}
                  onPointerDown={press.onPointerDown(r)}
                  onPointerUp={press.cancel}
                  onPointerMove={press.cancel}
                  onPointerCancel={press.cancel}
                  onContextMenu={(e) => isMobile && e.preventDefault()}
                  className="group/row min-h-11 items-center gap-3 py-1.5 pointer-coarse:min-h-11"
                >
                  <r.Icon className="shrink-0" aria-hidden />
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-sm">
                      {r.ref.title}
                      {r.archived && <Archived />}
                    </div>
                    <div className="truncate text-xs text-fg-tertiary">{r.ref.meta}</div>
                  </div>
                  {r.badges.map((b) => (
                    <span key={b} className="shrink-0 rounded-full bg-layer-2 px-1.5 py-0.5 text-[10px] text-fg-secondary">
                      {b}
                    </span>
                  ))}
                  {isMobile ? (
                    <span
                      aria-hidden
                      className={cn('size-4 shrink-0 rounded-full border', on ? 'border-fg-accent bg-fg-accent' : 'border-border')}
                    />
                  ) : (
                    <button
                      type="button"
                      aria-label={on ? `Deselect ${r.ref.title}` : `Select ${r.ref.title}`}
                      aria-pressed={on}
                      onPointerDown={(e) => e.stopPropagation()}
                      onClick={(e) => {
                        e.stopPropagation()
                        toggle(r)
                      }}
                      className={cn(
                        'flex size-4 shrink-0 items-center justify-center rounded-sm border',
                        on ? 'border-fg-accent bg-fg-accent text-primary-foreground' : 'border-border opacity-0 group-data-[selected=true]/row:opacity-100 hover:opacity-100',
                      )}
                    >
                      {on && <Check className="size-3" />}
                    </button>
                  )}
                </CommandItem>
              )
            })}
            {g.footer}
          </CommandGroup>
        ),
      )}
    </CommandList>
  )

  const chips =
    selected.size > 0 ? (
      <div className="flex min-w-0 flex-1 flex-wrap items-center gap-1 text-xs">
        <span className="text-fg-secondary">{selected.size} selected</span>
        {[...selected.values()].map((r) => (
          <span key={key(r)} className="flex max-w-40 items-center gap-1 rounded-full bg-layer-2 px-2 py-0.5">
            <span className="truncate">{r.title}</span>
            <button type="button" aria-label={`Deselect ${r.title}`} onClick={() => setSelected((s) => { const n = new Map(s); n.delete(key(r)); return n })}>
              ×
            </button>
          </span>
        ))}
      </div>
    ) : null

  const command = (
    <Command
      label="Attach"
      shouldFilter={false}
      loop
      disablePointerSelection={isMobile}
      value={current?.key ?? ''}
      onValueChange={setHighlighted}
      onKeyDown={onKeyDown}
      className="min-h-0 flex-1 bg-transparent"
    >
      {!quiet && (
        <div className="flex min-h-11 shrink-0 items-center gap-2 border-b px-3">
          <Search className="size-4 shrink-0 opacity-50" aria-hidden />
          <CommandPrimitive.Input
            ref={inputRef}
            value={typed}
            onValueChange={setTyped}
            placeholder="Search notes, knowledge, artifacts, chats, meetings…"
            aria-label="Search things to attach"
            autoCapitalize="off"
            autoCorrect="off"
            spellCheck={false}
            className="h-11 w-full bg-transparent text-base outline-hidden placeholder:text-muted-foreground md:text-sm"
          />
        </div>
      )}
      <div className="shrink-0 border-b">{tabs}</div>
      <div aria-live="polite" className="sr-only">
        {announce}
      </div>
      {list}
    </Command>
  )

  if (isMobile) {
    return (
      <>
        <Sheet open={open} onOpenChange={(o) => !o && onClose()}>
          <SheetContent side="bottom" showCloseButton={false} className="max-h-[90vh] gap-0 p-0 motion-reduce:animate-none">
            <SheetTitle className="sr-only">Attach</SheetTitle>
            <SheetDescription className="sr-only">Tap to select; Attach sends the selection to the composer. Long-press a row for a preview.</SheetDescription>
            <div className="mx-auto mt-2 h-1 w-10 shrink-0 rounded-full bg-border" aria-hidden />
            {command}
            <MobileFooter inset={inset} count={selected.size} onAttach={() => attach()} onClose={onClose} />
          </SheetContent>
        </Sheet>
        <Sheet open={pressed !== null} onOpenChange={(o) => !o && setPressed(null)}>
          <SheetContent side="bottom" className="gap-3 px-4 pb-6">
            {pressed && (
              <>
                <SheetTitle className="flex items-center gap-2">
                  <pressed.Icon className="size-4 text-fg-secondary" /> {pressed.ref.title}
                </SheetTitle>
                <SheetDescription>{pressed.ref.meta || KIND_LABEL[pressed.ref.kind]}</SheetDescription>
                <Thumb artifact={pressed.artifact} />
                <div className="flex gap-2">
                  <Button
                    variant="outline"
                    className="flex-1"
                    onClick={() => {
                      setPressed(null)
                      onClose()
                      openRef(pressed.ref)
                    }}
                  >
                    Open
                  </Button>
                  <Button
                    className="flex-1"
                    onClick={() => {
                      setPressed(null)
                      onAttach([...selected.values(), pressed.ref])
                    }}
                  >
                    Attach
                  </Button>
                </div>
              </>
            )}
          </SheetContent>
        </Sheet>
      </>
    )
  }

  const showPreview = wide && preview && !quiet
  return (
    <Popover open={open} onOpenChange={(o) => !o && onClose()}>
      <PopoverAnchor virtualRef={anchor} />
      <PopoverContent
        side="top"
        align="start"
        sideOffset={8}
        collisionPadding={8}
        onOpenAutoFocus={(e) => {
          e.preventDefault()
          if (!quiet) inputRef.current?.focus()
        }}
        onCloseAutoFocus={(e) => e.preventDefault()}
        // Focus legitimately sits outside: the textarea in quiet mode, the `+` button the menu hands focus back to.
        onFocusOutside={(e) => e.preventDefault()}
        className={cn('flex h-[440px] max-h-[calc(100dvh-6rem)] flex-row p-0', showPreview ? 'w-[min(840px,calc(100vw-2rem))]' : 'w-[min(560px,calc(100vw-2rem))]')}
      >
        <div className="flex min-w-0 flex-1 flex-col">
          {command}
          <div className="flex min-h-10 shrink-0 items-center gap-2 border-t px-3 py-1.5 text-xs text-fg-tertiary">
            {chips ?? <span className="min-w-0 flex-1 truncate">⏎ attach · ⌘⏎ add · ⇥ tabs · → preview</span>}
            <Button size="sm" disabled={selected.size === 0} onClick={() => attach()}>
              Done
            </Button>
          </div>
        </div>
        {showPreview && (
          <aside className="flex w-[280px] shrink-0 flex-col gap-2 overflow-y-auto border-l p-3 text-sm" aria-label="Preview">
            {current ? (
              <>
                <div className="flex items-center gap-2 font-medium">
                  <current.Icon className="size-4 shrink-0 text-fg-secondary" />
                  <span className="min-w-0 break-words">{current.ref.title}</span>
                </div>
                <Thumb artifact={current.artifact} />
                <div className="text-xs text-fg-secondary break-words">{current.ref.meta}</div>
                <div className="text-xs text-fg-tertiary">{KIND_LABEL[current.ref.kind]}</div>
                {current.badges.length > 0 && <div className="text-xs text-fg-tertiary">{current.badges.join(' · ')}</div>}
                <Button variant="outline" size="sm" className="mt-auto" onClick={() => { onClose(); openRef(current.ref) }}>
                  Open
                </Button>
              </>
            ) : (
              <div className="m-auto text-xs text-fg-tertiary">Highlight a row to preview it.</div>
            )}
          </aside>
        )}
      </PopoverContent>
    </Popover>
  )
}

/** An artifact's preview PNG or the image itself (§7.5); nothing for other kinds. */
function Thumb({ artifact: a }: { artifact?: ArtifactRecord }) {
  if (!a) return null
  const path = a.preview ?? (a.kind === 'image' || a.kind === 'svg' ? a.path : null)
  if (!path) return null
  return <Img path={path} className="max-h-40 w-full rounded-md object-contain object-top bg-layer-2" />
}

/** Sticky footer above the keyboard: **Attach N**, or Close with nothing selected (§7.3). */
function MobileFooter({ inset, count, onAttach, onClose }: { inset: number; count: number; onAttach(): void; onClose(): void }) {
  return (
    <div className="shrink-0 border-t p-3" style={{ paddingBottom: `calc(0.75rem + ${inset}px)` }}>
      {count > 0 ? (
        <Button className="min-h-11 w-full" onClick={onAttach}>
          Attach {count}
        </Button>
      ) : (
        <Button variant="outline" className="min-h-11 w-full" onClick={onClose}>
          Close
        </Button>
      )}
    </div>
  )
}
