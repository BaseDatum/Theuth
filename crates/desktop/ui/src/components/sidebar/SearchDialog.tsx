import { useQuerySupport } from '@/rpc/capabilities'
import { useQueries, useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate, useParams } from '@tanstack/react-router'
import { Command as CommandPrimitive } from 'cmdk'
import { Check, ChevronDown, CircleHelp, FileText, History, MessageSquare, Mic, Paperclip, RefreshCw, Search, SquarePen, StickyNote, X } from 'lucide-react'
import { type ReactNode, useEffect, useRef, useState } from 'react'
import { VoiceWave, useVoiceSource } from '@/components/voice/VoiceWave'
import { Button } from '@/components/ui/button'
import { Command, CommandGroup, CommandItem, CommandList } from '@/components/ui/command'
import { machineRows, openNewChatOn } from '@/components/chat/NewChatPane'
import { useMesh } from '@/components/tasks/queries'
import { Dialog, DialogClose, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { type AttachRef, attachTo, chatName, cleanTitle, toRef } from '@/lib/attach'
import { previewText, stripSnippet } from '@/lib/channel-origin'
import * as dictation from '@/lib/dictation'
import { queryKeys, useApi } from '@/lib/rpc'
import * as speech from '@/lib/speech'
import { tapOrLevels } from '@/lib/visual-source'
import {
  CATEGORIES,
  CATEGORY_LABEL,
  DEFAULT_LIMITS,
  LIMITS,
  MAX_QUERY_CHARS,
  type Limits,
  type SearchDestination,
  type SearchRequest,
  type SearchView,
  canonical,
  indexing,
  patch,
  useSearchState,
} from '@/lib/search'
import { SearchQueue } from '@/lib/search-queue'
import { LOCAL, useEngines } from '@/link/engines'
import { cn } from '@/lib/utils'
import type { ArtifactSearchHit } from "@/rpc/bindings/method.rpc_2dcompose.output"
import type { ConceptSearchHit } from "@/rpc/bindings/method.rpc_2dcompose.output"
import type { HistorySearchHit } from "@/rpc/bindings/method.rpc_2dcompose.output"
import type { MeetingSearchHit } from "@/rpc/bindings/method.rpc_2dcompose.output"
import type { NoteSearchHit } from "@/rpc/bindings/method.rpc_2dcompose.output"
import type { BlockHit } from "@/rpc/bindings/method.document.output"
import type { SearchCategory } from '@/rpc/bindings/method.rpc_2dcompose.input'
import type { SearchGroup } from "@/rpc/bindings/method.rpc_2dcompose.output"
import type { MatchRange } from "@/rpc/bindings/method.artifact.output"
import type { SearchHit } from "@/rpc/bindings/method.rpc_2dcompose.output"
import type { SessionSummary } from "@/rpc/bindings/method.agent.output"
import type { SearchResponse } from "@/rpc/bindings/method.rpc_2dcompose.output"
import { Archived, Notice, Notices, Section, Snippet, status } from '@/components/search/parts'
import { ago } from './AgentTree'

const INDEXING_POLL_MS = 500
const RECENT = 10

const ICON: Record<SearchCategory, typeof Search> = {
  sessions: MessageSquare,
  artifacts: FileText,
  knowledge: Search,
  knowledge_history: History,
  notes: StickyNote,
  meetings: Mic,
  // 144 §7.4: rows for the two block-level groups land with the notebook UI (W4).
  sources: FileText,
  notebooks: StickyNote,
}

/** Show more steps 10 → 30 → 50; the cap stays put. */
const NEXT_LIMIT: Record<number, number> = { 10: 30, 30: 50 }

const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e))

/** The visible search trigger nearest DOM order: the offcanvas sidebar keeps its trigger mounted off-screen, so test the viewport, not just display. */
function visibleTrigger(): HTMLElement | undefined {
  return [...document.querySelectorAll<HTMLElement>('[data-search-trigger]')].find((el) => {
    const r = el.getBoundingClientRect()
    return r.width > 0 && r.right > 0 && r.left < window.innerWidth
  })
}

/** One activatable row: cmdk value/React key is the stable source identity (49 §2.3), never a row index. `ref` is the row as an attachment (66 §7.8). */
type Item = { key: string; dest: SearchDestination; node: ReactNode; ref: AttachRef }

function items(g: SearchGroup): Item[] {
  switch (g.category) {
    case 'sessions':
      return g.page.hits.map((h) => ({
        key: `sessions:${h.session}:${h.seq ?? 'title'}`,
        dest: { category: 'sessions', session: h.session, seq: h.seq },
        node: <SessionRow hit={h} />,
        ref: toRef({ kind: 'session', hit: h }),
      }))
    case 'artifacts':
      return g.page.hits.map((h) => ({
        key: `artifacts:${h.artifact.call_id}`,
        dest: { category: 'artifacts', callId: h.artifact.call_id },
        node: <ArtifactRow hit={h} />,
        ref: toRef({ kind: 'artifact', artifact: h.artifact }),
      }))
    case 'knowledge':
      return g.page.hits.map((h) => ({
        key: `knowledge:${h.meta.id}`,
        dest: { category: 'knowledge', id: h.meta.id },
        node: <ConceptRow hit={h} />,
        ref: toRef({ kind: 'knowledge', meta: h.meta }),
      }))
    case 'knowledge_history':
      return g.page.hits.map((h) => ({
        key: `knowledge_history:${h.commit}:${h.id}`,
        dest: { category: 'knowledge_history', id: h.id, commit: h.commit, snapshot: h.snapshot_commit },
        node: <HistoryRow hit={h} />,
        ref: toRef({ kind: 'knowledge', revision: h }),
      }))
    case 'notes':
      return g.page.hits.map((h) => ({
        key: `notes:${h.doc.id}:${h.block}`,
        dest: { category: 'notes', id: h.doc.id, block: h.block },
        node: <NoteRow hit={h} />,
        ref: toRef({ kind: 'note', doc: h.doc }),
      }))
    case 'meetings':
      return g.page.hits.map((h) => ({
        key: `meetings:${h.meeting.id}`,
        dest: { category: 'meetings', id: h.meeting.id },
        node: <MeetingRow hit={h} />,
        ref: toRef({ kind: 'meeting', meeting: h.meeting }),
      }))
    case 'sources':
      return g.page.hits.map((h) => ({
        key: `sources:${h.doc}:${h.block}`,
        dest: { category: 'sources', id: h.doc, block: h.block },
        node: <BlockRow hit={h} kind="source" />,
        ref: toRef({ kind: 'source', hit: h }),
      }))
    case 'notebooks':
      return g.page.hits.map((h) => ({
        key: `notebooks:${h.doc}:${h.block}`,
        dest: { category: 'notebooks', id: h.doc, block: h.block },
        node: <BlockRow hit={h} kind="notebook" />,
        ref: toRef({ kind: 'notebook', hit: h }),
      }))
  }
}

type Props = {
  open: boolean
  onOpenChange(open: boolean): void
  /** The latest `openSearch` request; a new object per event. */
  request?: SearchRequest
}

/**
 * ⌘K search (49 §2, §5): one query over four independently selected categories, grouped results with
 * per-group limits, states and provenance; activation opens the exact source and remembers the view for
 * Back to results. Chips live outside cmdk's key handling; every request is identified by its query key.
 */
export function SearchDialog({ open, onOpenChange, request }: Props) {
  const canQuery1 = useQuerySupport(["search"])
  const canQuery1_2 = useQuerySupport(["search"])
  const canQuery1_3 = useQuerySupport(["search"])
  const canQuery1_4 = useQuerySupport(["list_sessions"])
  const api = useApi()
  const qc = useQueryClient()
  const navigate = useNavigate()
  // 66 §7.8: with a chat open, every row offers "Attach to current chat".
  const currentChat = useParams({ strict: false }).session ?? null
  const remembered = useSearchState((s) => s.categories)
  const setRemembered = useSearchState((s) => s.setCategories)
  const leave = useSearchState((s) => s.leave)

  const [typed, setTyped] = useState('')
  const [debounced, setDebounced] = useState('')
  const [composing, setComposing] = useState(false)
  /** One-open selection (`/sessions`, Back to results); `null` means the remembered set. */
  const [override, setOverride] = useState<SearchCategory[] | null>(null)
  const [limits, setLimits] = useState<Limits>(DEFAULT_LIMITS)
  const [selected, setSelected] = useState('')
  const [refreshing, setRefreshing] = useState<SearchCategory[] | null>(null)
  const [refreshError, setRefreshError] = useState<string | null>(null)
  const [help, setHelp] = useState(false)
  const inputRef = useRef<HTMLInputElement>(null)
  const listRef = useRef<HTMLDivElement>(null)
  const opener = useRef<HTMLElement | null>(null)
  const activated = useRef(false)
  // Dictation into the query (54; `dictation.useBox`): the finished text lands through `setTyped` like typing.
  const voice = speech.useVoice()
  const mic = useEngines((s) => s.active) !== LOCAL
  const dict = dictation.useBox(api, typed, setTyped, inputRef, mic)
  // 82 §B as in the composer pill: the dictation tap's analyser (remote engine) or frames from the engine's level packets.
  const dictSource = useVoiceSource(dict.dictating && dict.dict.phase === 'dictating', () => tapOrLevels(dictation.tap(), dictation.onLevels))
  const wasOpen = useRef(false)
  const pendingScroll = useRef<number | null>(null)
  const refreshGen = useRef(0)
  /** Dialog-open generation (49 §5.3): every request of this open is keyed by it, so a late success or error from a previous open lands under a key nothing observes. */
  const opens = useRef(0)
  const [gen, setGen] = useState(0)
  /** This surface's request discipline (lib/search-queue): resetting debounce, one `search` in flight, latest settled query pending. */
  const queue = useRef(new SearchQueue()).current

  const cats = override ?? remembered
  const query = typed.trim()
  /** UTF-16 length bounds the scalar count; only an over-long string pays for the exact count. */
  const tooLong = query.length > MAX_QUERY_CHARS && Array.from(query).length > MAX_QUERY_CHARS
  /** Edits not yet requested: rows on screen belong to the previous query and must not open. */
  const pending = query !== debounced
  const searching = open && debounced !== '' && cats.length > 0 && !tooLong

  useEffect(() => {
    if (composing || query === debounced) return
    const settle = (q: string) => {
      setDebounced(q)
      setLimits(DEFAULT_LIMITS)
    }
    // An emptied box settles at once: the pending query is dropped and the list clears without a debounce.
    if (query === '') {
      queue.clear()
      settle('')
      return
    }
    queue.type(query, settle)
    return () => queue.cancel()
  }, [query, debounced, composing, queue])
  useEffect(() => {
    if (!open) queue.clear()
  }, [open, queue])

  const refresh = async (which: SearchCategory[]) => {
    const gen = ++refreshGen.current
    setRefreshing(which)
    setRefreshError(null)
    try {
      await api.search_refresh(which)
    } catch (e) {
      if (gen === refreshGen.current) setRefreshError(errorText(e))
    } finally {
      // Owner caches changed whether or not this open still cares; only the status line is generation-gated.
      void qc.invalidateQueries({ queryKey: queryKeys.searchAll })
      if (gen === refreshGen.current) setRefreshing(null)
    }
  }

  /** Put a restored view's groups back under their query keys, keeping their original fetch time. */
  const seed = (view: SearchView, gen: number) => {
    const base = queryKeys.search(view.query, view.categories, LIMITS[0], gen)
    if (!qc.getQueryData(base)) qc.setQueryData(base, { groups: view.groups }, { updatedAt: view.at })
    for (const g of view.groups) {
      const limit = view.limits[g.category]
      if (limit === LIMITS[0]) continue
      const key = queryKeys.search(view.query, [g.category], limit, gen)
      if (!qc.getQueryData(key)) qc.setQueryData(key, { groups: [g] }, { updatedAt: view.at })
    }
  }

  // Apply the opening request: blank on an ordinary open, the saved view on Back to results, `/search q` or
  // `/sessions` prefills. Handlers reset limits/selection, not effects, so a restore is not clobbered.
  useEffect(() => {
    const opening = open && !wasOpen.current
    wasOpen.current = open
    if (!open) {
      refreshGen.current++
      setRefreshing(null)
      setRefreshError(null)
      return
    }
    // `activated` must survive until Radix's close-autofocus fires after the exit animation; clear it here.
    if (opening) {
      activated.current = false
      setGen(++opens.current)
    }
    const req = request ?? {}
    if (!opening && !req.restore && req.query === undefined && !req.categories) return
    const view = req.restore ? useSearchState.getState().view : null
    if (view) {
      setTyped(view.query)
      setDebounced(view.query)
      setOverride(view.categories)
      setLimits(view.limits)
      setSelected(view.selected ?? '')
      pendingScroll.current = view.scroll
      seed(view, opens.current)
    } else {
      const q = req.query ?? ''
      setTyped(q)
      setDebounced(q.trim())
      setOverride(req.categories ? canonical(req.categories) : null)
      setLimits(DEFAULT_LIMITS)
      setSelected('')
    }
    const effective = view?.categories ?? (req.categories ? canonical(req.categories) : useSearchState.getState().categories)
    if (opening && effective.includes('knowledge')) void refresh(['knowledge'])
    // eslint-disable-next-line react-hooks/exhaustive-deps -- request identity is the trigger
  }, [open, request])

  const baseKey = queryKeys.search(debounced, cats, LIMITS[0], gen)
  const base = useQuery({
    queryKey: baseKey,
    queryFn: () => queue.run(debounced, () => api.search(debounced, cats, LIMITS[0])),
    enabled: canQuery1 && (searching),
  })
  const moreCats = cats.filter((c) => limits[c] > LIMITS[0])
  const more = useQueries({
    queries: moreCats.map((c) => ({
      queryKey: queryKeys.search(debounced, [c], limits[c], gen),
      queryFn: () => queue.run(debounced, () => api.search(debounced, [c], limits[c])),
      enabled: canQuery1_2 && (searching),
      // A one-category page re-asks only itself while it indexes.
      refetchInterval: (q: { state: { data?: { groups: SearchGroup[] } } }) =>
        q.state.data?.groups[0]?.page.state === 'indexing' ? INDEXING_POLL_MS : false,
      refetchIntervalInBackground: false,
    })),
  })
  // 49 §4.5: while some selected groups build, reissue only those every 500 ms under the same request identity and
  // fold each answer into the base page; ready groups are never re-matched. When every group indexes this is the
  // base key itself, so the base simply refetches. Disabling (close, edit, chip, all settled) stops the poll.
  const building = indexing(cats, base.data?.groups, limits)
  useQuery({
    queryKey: queryKeys.search(debounced, building, LIMITS[0], gen),
    queryFn: async () => {
      const fresh = await queue.run(debounced, () => api.search(debounced, building, LIMITS[0]))
      qc.setQueryData<SearchResponse>(baseKey, (old) => old && { groups: patch(old.groups, fresh.groups) })
      return fresh
    },
    enabled: canQuery1_3 && (searching && building.length > 0),
    refetchInterval: INDEXING_POLL_MS,
    refetchIntervalInBackground: false,
  })
  const recent = useQuery({
    queryKey: queryKeys.sessions,
    queryFn: () => api.list_sessions(),
    enabled: canQuery1_4 && (open && debounced === '' && cats.includes('sessions')),
  })
  // 183 §12.5: "New chat on <machine>" for each paired engine (this machine is the plain New chat).
  const machines = machineRows(useMesh().data).filter((m): m is typeof m & { id: string } => m.id !== null)

  /**
   * Per selected category: the displayed group (a Show more page wins once it arrived), that page's fetch, and
   * whether its rows may open. Rows are non-activatable while their query is unrequested (`pending`) or while the
   * page they came from is being refetched (invalidation, Refresh, a cached page revisited): old data of the same
   * identity is shown dimmed until the new answer lands.
   */
  const display = cats.map((c) => {
    const m = more[moreCats.indexOf(c)]
    const group = m?.data?.groups[0] ?? base.data?.groups.find((g) => g.category === c)
    return { category: c, group, more: m, stale: pending || (m?.data ? m.isFetching : base.isFetching) }
  })

  useEffect(() => {
    if (pendingScroll.current === null || !base.data || !listRef.current) return
    listRef.current.scrollTop = pendingScroll.current
    pendingScroll.current = null
  }, [base.data])

  const select = (next: SearchCategory[]) => {
    setOverride(null)
    setRemembered(next)
    setLimits(DEFAULT_LIMITS)
  }

  const go = (dest: SearchDestination) => {
    leave(
      {
        query: debounced,
        categories: cats,
        limits,
        groups: display.flatMap((d) => (d.group ? [d.group] : [])),
        at: base.dataUpdatedAt || Date.now(),
        selected,
        scroll: listRef.current?.scrollTop ?? 0,
      },
      { query: debounced, destination: dest },
    )
    activated.current = true
    onOpenChange(false)
    switch (dest.category) {
      case 'sessions':
        return void navigate({
          to: '/c/$session',
          params: { session: dest.session },
          search: dest.seq === null ? {} : { seq: dest.seq },
        })
      case 'artifacts':
        return void navigate({ to: '/artifacts/$callId', params: { callId: dest.callId } })
      case 'knowledge':
        return void navigate({ to: '/knowledge/$', params: { _splat: dest.id }, search: {} })
      case 'knowledge_history':
        return void navigate({
          to: '/knowledge/$',
          params: { _splat: dest.id },
          search: dest.snapshot === null ? { change: dest.commit } : { change: dest.commit, snapshot: dest.snapshot },
        })
      case 'notes':
        return void navigate({ to: '/notes/$id', params: { id: dest.id }, search: {}, hash: dest.block })
      case 'meetings':
        return void navigate({ to: '/meetings/$id', params: { id: dest.id } })
      case 'sources':
        return void navigate({ to: '/sources/$id', params: { id: dest.id }, search: {}, hash: dest.block })
      case 'notebooks':
        return void navigate({ to: '/notebooks/$id', params: { id: dest.id }, search: {} })
    }
  }

  const flush = () => {
    if (!pending) return
    setDebounced(query)
    setLimits(DEFAULT_LIMITS)
  }

  const allSelected = cats.length === CATEGORIES.length
  const settled = searching && !!base.data && more.every((m) => m.data || m.error)
  const hits = display.reduce((n, d) => n + (d.group?.page.hits.length ?? 0), 0)
  const complete = display.every((d) => d.group?.page.state === 'ready' && d.group.page.coverage === 'complete')
  const announce = !settled
    ? ''
    : display
        .map((d) => `${CATEGORY_LABEL[d.category]}: ${d.group ? status(d.group.page) : 'no answer'}`)
        .join('. ')

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        showCloseButton={false}
        onOpenAutoFocus={(e) => {
          opener.current = document.activeElement instanceof HTMLElement ? document.activeElement : null
          e.preventDefault()
          inputRef.current?.focus()
        }}
        onCloseAutoFocus={(e) => {
          // Activation hands focus to the destination; dismissal returns it to the opener, or to a trigger
          // still on screen when the opener left with the mobile drawer.
          e.preventDefault()
          if (activated.current) return
          const el = opener.current
          const back = el?.isConnected && el.getBoundingClientRect().width > 0 ? el : visibleTrigger()
          back?.focus()
        }}
        className={cn(
          'top-0 left-0 flex h-band w-screen max-w-none translate-x-0 translate-y-0 flex-col gap-0 rounded-none border-0 p-0',
          'pt-[var(--safe-top)] pr-[var(--safe-right)] pb-[var(--safe-bottom)] pl-[var(--safe-left)]',
          'md:top-[50%] md:left-[50%] md:h-auto md:max-h-[min(720px,calc(100dvh-4rem))] md:w-full md:max-w-[720px] md:translate-x-[-50%] md:translate-y-[-50%] md:rounded-lg md:border',
          'motion-reduce:animate-none motion-reduce:transition-none',
        )}
      >
        <div className="flex h-11 shrink-0 items-center gap-1 border-b pr-1 pl-4">
          <DialogTitle className="mr-auto text-sm font-medium">Search</DialogTitle>
          <DialogDescription className="sr-only">
            Search sessions, artifacts, knowledge and knowledge history on this node. Arrow keys move
            between results; Enter opens the selected result.
          </DialogDescription>
          {/* Mobile: the footer is hidden, so Refresh and help live here as icons. */}
          <Button
            variant="ghost"
            size="icon"
            className="md:hidden"
            disabled={refreshing !== null || cats.length === 0}
            aria-busy={refreshing !== null}
            aria-label={refreshing ? 'Refreshing search sources' : 'Refresh search sources'}
            onClick={() => void refresh(cats)}
          >
            <RefreshCw className={cn('size-[18px]', refreshing && 'motion-safe:animate-spin')} aria-hidden />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            className="md:hidden"
            aria-label="Search help"
            aria-expanded={help}
            aria-controls="search-help"
            onClick={() => setHelp((h) => !h)}
          >
            <CircleHelp className="size-[18px]" aria-hidden />
          </Button>
          <DialogClose asChild>
            <Button variant="ghost" size="icon" aria-label="Close search">
              <X className="size-[18px]" />
            </Button>
          </DialogClose>
        </div>
        <Command
          label="Search"
          shouldFilter={false}
          loop
          disablePointerSelection
          value={selected}
          onValueChange={setSelected}
          className="min-h-0 flex-1 rounded-none bg-transparent"
        >
          <div className="flex min-h-11 shrink-0 items-center gap-2 border-b px-3">
            <Search className="size-4 shrink-0 opacity-50" aria-hidden />
            <CommandPrimitive.Input
              ref={inputRef}
              value={dict.shown}
              readOnly={dict.dictating}
              onValueChange={setTyped}
              onCompositionStart={() => setComposing(true)}
              onCompositionEnd={() => setComposing(false)}
              onKeyDown={(e) => {
                // Home/End edit the query (cmdk would jump rows); Enter first flushes a pending edit and never
                // opens a row from the previous query.
                if (e.key === 'Home' || e.key === 'End') e.stopPropagation()
                if (e.key === 'Enter' && pending && !e.nativeEvent.isComposing) {
                  e.stopPropagation()
                  flush()
                }
              }}
              placeholder="Search…"
              aria-label="Search query"
              aria-invalid={tooLong || undefined}
              aria-describedby="search-query-hint"
              autoCapitalize="off"
              autoCorrect="off"
              spellCheck={false}
              className="h-11 w-full bg-transparent text-base outline-hidden placeholder:text-muted-foreground md:text-sm"
            />
            {typed !== '' && (
              <Button
                variant="ghost"
                size="sm"
                aria-label="Clear query"
                className="pointer-coarse:min-h-11"
                onClick={() => {
                  setTyped('')
                  setDebounced('')
                  setLimits(DEFAULT_LIMITS)
                  inputRef.current?.focus()
                }}
              >
                Clear
              </Button>
            )}
            {voice && (
              <Button
                variant="ghost"
                className={cn(
                  'shrink-0 rounded-full',
                  dict.dictating ? 'h-8 w-20 bg-transparent p-0' : 'size-8',
                  dict.reason && !dict.dictating && 'opacity-50',
                )}
                aria-label={dict.dictating ? 'Stop dictation' : 'Dictate'}
                aria-disabled={dict.reason !== null && !dict.dictating}
                title={dict.reason ?? undefined}
                onClick={dict.toggle}
              >
                {dict.dictating ? <VoiceWave state="listening" source={dictSource} label="Dictation level" className="h-6 w-16" /> : <Mic className="size-4" />}
              </Button>
            )}
          </div>
          {/* Source controls: keydown does not reach cmdk's root, so chip/menu Enter/Space/arrows stay local. */}
          <div
            className="flex shrink-0 flex-wrap items-center gap-2 border-b px-3 py-2"
            onKeyDown={(e) => e.stopPropagation()}
          >
            <span id="search-in" className="text-xs text-fg-tertiary">
              Search in
            </span>
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button variant="outline" size="sm" className="md:hidden">
                  {allSelected ? 'All' : cats.length === 0 ? 'Nothing' : cats.map((c) => CATEGORY_LABEL[c]).join(', ')}
                  <ChevronDown aria-hidden />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="start">
                <DropdownMenuCheckboxItem
                  checked={allSelected}
                  onSelect={(e) => e.preventDefault()}
                  onCheckedChange={() => {
                    if (!allSelected) select([...CATEGORIES])
                  }}
                >
                  All
                </DropdownMenuCheckboxItem>
                <DropdownMenuSeparator />
                {CATEGORIES.map((c) => (
                  <DropdownMenuCheckboxItem
                    key={c}
                    checked={cats.includes(c)}
                    onSelect={(e) => e.preventDefault()}
                    onCheckedChange={(on) => select(canonical(on ? [...cats, c] : cats.filter((x) => x !== c)))}
                  >
                    {CATEGORY_LABEL[c]}
                  </DropdownMenuCheckboxItem>
                ))}
              </DropdownMenuContent>
            </DropdownMenu>
            <button
              type="button"
              aria-pressed={allSelected}
              onClick={() => {
                if (!allSelected) select([...CATEGORIES])
              }}
              className={cn(chip(allSelected), 'hidden md:inline-flex')}
            >
              {allSelected && <Check className="size-3.5" aria-hidden />}
              All
            </button>
            <ToggleGroup
              type="multiple"
              aria-labelledby="search-in"
              value={cats}
              onValueChange={(v) => select(canonical(v as SearchCategory[]))}
              spacing={2}
              className="hidden flex-wrap md:flex"
            >
              {CATEGORIES.map((c) => {
                const on = cats.includes(c)
                const d = display.find((x) => x.category === c)
                const failed = d?.group && d.group.page.state !== 'ready' && d.group.page.state !== 'indexing'
                return (
                  <ToggleGroupItem
                    key={c}
                    value={c}
                    aria-label={CATEGORY_LABEL[c]}
                    aria-describedby={failed ? `search-state-${c}` : undefined}
                    className={chip(on)}
                  >
                    {on && <Check className="size-3.5" aria-hidden />}
                    {CATEGORY_LABEL[c]}
                  </ToggleGroupItem>
                )
              })}
            </ToggleGroup>
            {refreshing?.includes('knowledge') && (
              <span className="text-xs text-fg-tertiary" role="status">
                Updating knowledge…
              </span>
            )}
          </div>
          {refreshError && (
            <div role="alert" className="shrink-0 border-b px-3 py-2 text-xs text-destructive">
              Refresh failed: {refreshError}
            </div>
          )}
          {help && (
            <div id="search-help" className="shrink-0 space-y-1 border-b px-3 py-2 text-xs text-fg-secondary">
              <p>
                Fuzzy by default; space separates terms. <code>'exact</code>, <code>^prefix</code>,{' '}
                <code>suffix$</code>, <code>!not</code>. Smart case: lowercase matches any case.
              </p>
              <p>
                Each term matches one candidate text at a time: a title, one message, one artifact body, or one
                side of a knowledge change. <code>!old</code> excludes candidates containing “old”, not every record
                that mentions it somewhere.
              </p>
              <p>
                Knowledge is synced when search opens; edits made while it stays open need Refresh. Artifacts and
                history index on first use.
              </p>
            </div>
          )}
          <div id="search-query-hint" className="sr-only">
            Fuzzy match; 'exact, ^prefix, suffix$, !not. Each term applies to one title or one body at a time.
          </div>
          <div aria-live="polite" className="sr-only">
            {announce}
          </div>
          <CommandList
            ref={listRef}
            className="max-h-none min-h-0 flex-1 overscroll-contain md:min-h-[200px]"
            aria-busy={searching && base.isFetching}
          >
            {cats.length === 0 ? (
              <Notice>Select at least one category.</Notice>
            ) : tooLong ? (
              <Notice tone="error">Query is longer than {MAX_QUERY_CHARS} characters.</Notice>
            ) : debounced === '' ? (
              <>
                <Notice>Type to search {cats.map((c) => CATEGORY_LABEL[c].toLowerCase()).join(', ')}.</Notice>
                {cats.includes('sessions') && recent.data && recent.data.length > 0 && (
                  <CommandGroup heading="Recent sessions · not matches">
                    {recent.data.slice(0, RECENT).map((s) => (
                      <CommandItem
                        key={s.id}
                        value={`recent:${s.id}`}
                        disabled={pending}
                        onSelect={() => go({ category: 'sessions', session: s.id, seq: null })}
                        className="min-h-11 gap-3"
                      >
                        <MessageSquare aria-hidden />
                        <span className="min-w-0 flex-1 truncate">
                          {s.name ? `↳ ${s.name}` : chatName(s)}
                          {s.archived && <Archived />}
                        </span>
                        <span className="text-xs text-fg-tertiary">{ago(s.updated_at)}</span>
                      </CommandItem>
                    ))}
                  </CommandGroup>
                )}
                {machines.length > 0 && (
                  <CommandGroup heading="New chat on">
                    {machines.map((m) => (
                      <CommandItem
                        key={m.id}
                        value={`new-chat-on:${m.id}`}
                        disabled={!m.online}
                        onSelect={() => {
                          onOpenChange(false)
                          openNewChatOn(navigate, m.id)
                        }}
                        className="min-h-11 gap-3"
                      >
                        <SquarePen aria-hidden />
                        <span className="min-w-0 flex-1 truncate">New chat on {m.name}</span>
                        <span className="text-xs text-fg-tertiary">{m.status}</span>
                      </CommandItem>
                    ))}
                  </CommandGroup>
                )}
              </>
            ) : base.error ? (
              <Notice tone="error">
                Search failed: {errorText(base.error)}
                <Button variant="outline" size="sm" className="ml-2" onClick={() => void base.refetch()}>
                  Retry
                </Button>
              </Notice>
            ) : !base.data ? (
              <Notice>Searching…</Notice>
            ) : (
              <>
                {settled && hits === 0 && (
                  <Notice>
                    {complete
                      ? 'No matches in selected categories.'
                      : 'No matches shown; some selected content was not searched.'}
                  </Notice>
                )}
                {display.map(({ category, group, more: m, stale }) =>
                  group ? (
                    <Group
                      key={category}
                      group={group}
                      stale={stale}
                      limit={limits[category]}
                      loadingMore={!!m && !m.data && !m.error}
                      moreError={m?.error ? errorText(m.error) : null}
                      refreshing={!!refreshing?.includes(category)}
                      onMore={() => setLimits((l) => ({ ...l, [category]: NEXT_LIMIT[l[category]] ?? l[category] }))}
                      onRetryMore={() => void m?.refetch()}
                      onRetry={() => void refresh([category])}
                      onOpen={go}
                      onAttach={
                        currentChat
                          ? (ref) => {
                              attachTo(currentChat, ref)
                              onOpenChange(false)
                            }
                          : undefined
                      }
                    />
                  ) : (
                    <Section key={category} category={category} state="No answer for this category." />
                  ),
                )}
              </>
            )}
          </CommandList>
        </Command>
        <div className="hidden shrink-0 items-center gap-x-3 border-t px-3 py-2 text-xs text-fg-tertiary md:flex">
          <Button
            variant="ghost"
            size="sm"
            disabled={refreshing !== null || cats.length === 0}
            aria-busy={refreshing !== null}
            aria-label={refreshing ? 'Refreshing search sources' : 'Refresh search sources'}
            onClick={() => void refresh(cats)}
          >
            <RefreshCw className={cn('size-3.5', refreshing && 'motion-safe:animate-spin')} aria-hidden />
            {refreshing ? 'Refreshing…' : 'Refresh'}
          </Button>
          <span className="ml-auto">Arrow keys · Enter open</span>
          <Button variant="ghost" size="sm" aria-expanded={help} aria-controls="search-help" onClick={() => setHelp((h) => !h)}>
            Search help
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  )
}

const chip = (on: boolean) =>
  cn(
    'inline-flex h-8 items-center gap-1 rounded-full border px-3 text-xs transition-colors pointer-coarse:min-h-11 focus-visible:ring-[3px] focus-visible:ring-ring/50 focus-visible:outline-none data-[state=on]:bg-accent data-[state=on]:text-accent-foreground',
    on ? 'border-fg-accent bg-accent text-foreground' : 'border-border-subtle text-fg-secondary hover:bg-accent/60',
  )

function Group({
  group,
  stale,
  limit,
  loadingMore,
  moreError,
  refreshing,
  onMore,
  onRetryMore,
  onRetry,
  onOpen,
  onAttach,
}: {
  group: SearchGroup
  stale: boolean
  limit: number
  loadingMore: boolean
  moreError: string | null
  refreshing: boolean
  onMore(): void
  onRetryMore(): void
  onRetry(): void
  onOpen(dest: SearchDestination): void
  /** Present while a chat is open: the row action that attaches the hit to it (66 §7.8). */
  onAttach?(ref: AttachRef): void
}) {
  const { category, page } = group
  if (page.state !== 'ready') {
    return (
      <Section category={category} state={status(page)}>
        <Notices notices={page.notices} />
        {page.state === 'error' && (
          <div className="px-2 pb-2">
            <Button variant="outline" size="sm" onClick={onRetry} disabled={refreshing}>
              {refreshing ? 'Retrying…' : 'Retry'}
            </Button>
          </div>
        )}
        {page.state === 'indexing' && (
          <p className="px-2 pb-2 text-xs text-fg-tertiary" role="status">
            Indexing {CATEGORY_LABEL[category].toLowerCase()}; results appear when it finishes.
          </p>
        )}
      </Section>
    )
  }
  const Icon = ICON[category]
  const heading = `${CATEGORY_LABEL[category]} · ${status(page)}${page.coverage === 'partial' ? ' · some content not searched' : ''}${refreshing ? ' · updating…' : ''}`
  const atCap = !(limit in NEXT_LIMIT)
  return (
    <CommandGroup heading={heading} className={cn('pt-2', stale && 'opacity-50')}>
      <Notices notices={page.notices} />
      {items(group).map((it) => (
        <CommandItem
          key={it.key}
          value={it.key}
          disabled={stale}
          onSelect={() => onOpen(it.dest)}
          className="group/hit min-h-11 items-start gap-3 py-2"
        >
          <Icon className="mt-0.5" aria-hidden />
          {it.node}
          {onAttach && (
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label={`Attach ${it.ref.title} to current chat`}
              title="Attach to current chat"
              className="shrink-0 opacity-0 group-data-[selected=true]/hit:opacity-100 hover:opacity-100 focus-visible:opacity-100 pointer-coarse:opacity-100"
              onPointerDown={(e) => e.stopPropagation()}
              onClick={(e) => {
                e.stopPropagation()
                onAttach(it.ref)
              }}
            >
              <Paperclip />
            </Button>
          )}
        </CommandItem>
      ))}
      {page.has_more && (
        <div className="px-2 pt-1 pb-2 text-xs text-fg-tertiary">
          {atCap ? (
            'More matches; refine your query or categories.'
          ) : moreError ? (
            <>
              Show more failed: {moreError}
              <Button variant="outline" size="sm" className="ml-2" onClick={onRetryMore}>
                Retry
              </Button>
            </>
          ) : (
            <Button
              variant="outline"
              size="sm"
              className="pointer-coarse:min-h-11"
              disabled={loadingMore || stale}
              aria-busy={loadingMore}
              onClick={onMore}
            >
              {loadingMore ? 'Loading more…' : 'Show more'}
            </Button>
          )}
        </div>
      )}
    </CommandGroup>
  )
}

/** Title, up to two snippet lines and muted provenance; renders text only, never snippet HTML. */
function Lines({ title, snippet, meta }: { title: ReactNode; snippet: ReactNode; meta: (string | null)[] }) {
  return (
    <div className="min-w-0 flex-1">
      <div className="truncate text-sm">{title}</div>
      {snippet && <div className="line-clamp-2 text-xs break-words text-fg-secondary">{snippet}</div>}
      <div className="truncate text-xs text-fg-tertiary">{meta.filter(Boolean).join(' · ')}</div>
    </div>
  )
}

/** A session hit's highlighted text without a leading Slack marker (172); a marker-only text as its label. */
function SessionSnippet({ text, ranges }: { text: string; ranges: MatchRange[] }) {
  const s = stripSnippet(text, ranges)
  return s.text.trim() ? <Snippet text={s.text} ranges={s.ranges} /> : previewText(text)
}

function SessionRow({ hit }: { hit: SearchHit }) {
  // 183 R2: a wrapper-seeded title is shown cleaned, never highlighted raw.
  const raw = hit.title.trimStart().startsWith('<message ')
  const title = hit.kind === 'title' && !raw ? <SessionSnippet text={hit.snippet} ranges={hit.ranges} /> : previewText(cleanTitle(hit.title))
  return (
    <Lines
      title={
        <>
          {title}
          {hit.archived && <Archived />}
        </>
      }
      snippet={hit.kind === 'message' ? <SessionSnippet text={hit.snippet} ranges={hit.ranges} /> : null}
      meta={[
        hit.kind === 'title' ? 'title' : hit.role,
        hit.matches > 1 ? `${hit.matches} matching messages` : '',
        ago(hit.updated_at),
        hit.context_removed ? 'Removed from context' : '',
      ]}
    />
  )
}

const FIELD: Record<string, string> = { content: 'Content match', source: 'Source match' }

function ArtifactRow({ hit }: { hit: ArtifactSearchHit }) {
  const a = hit.artifact
  const m = hit.match
  // The sidebar keeps `list_sessions` warm; a chat the cache lacks shows no provenance rather than an id.
  const chat = useQueryClient().getQueryData<SessionSummary[]>(queryKeys.sessions)?.find((s) => s.id === a.session)
  // A title hit is shown highlighted in the title line itself; anything else keeps the title and shows the match below.
  const titled = m.field === 'title'
  return (
    <Lines
      title={titled ? <Snippet text={m.snippet} ranges={m.ranges} /> : a.title || a.name}
      snippet={titled ? null : <Snippet text={m.snippet} ranges={m.ranges} />}
      meta={[
        a.name,
        a.kind,
        `v${a.version}`,
        chat ? `in "${chatName(chat)}"` : '',
        ago(new Date(Number(a.created_at) * 1000).toISOString()),
        FIELD[m.field] ?? 'Metadata match',
        hit.content_coverage === 'metadata_only' ? 'Content not searchable' : '',
      ]}
    />
  )
}

function ConceptRow({ hit }: { hit: ConceptSearchHit }) {
  const m = hit.match
  return (
    <Lines
      title={hit.meta.title}
      snippet={<Snippet text={m.snippet} ranges={m.ranges} />}
      meta={[hit.meta.id, hit.meta.type, hit.meta.tags.join(', '), m.field === 'body' ? '' : 'Metadata match']}
    />
  )
}

const SIDE = { before: 'Before change', after: 'After change', metadata: 'Metadata match' } as const

function HistoryRow({ hit }: { hit: HistorySearchHit }) {
  const m = hit.match
  return (
    <Lines
      title={hit.title}
      snippet={<Snippet text={m.snippet} ranges={m.ranges} />}
      meta={[hit.id, hit.status, hit.commit.slice(0, 8), ago(hit.time), hit.message, SIDE[hit.side]]}
    />
  )
}

function NoteRow({ hit }: { hit: NoteSearchHit }) {
  const m = hit.match
  const titled = m.field === 'title'
  return (
    <Lines
      title={titled ? <Snippet text={m.snippet} ranges={m.ranges} /> : hit.doc.title || 'Untitled note'}
      snippet={titled ? null : <Snippet text={m.snippet} ranges={m.ranges} />}
      meta={['note', hit.heading && hit.heading !== m.snippet ? hit.heading : '', ago(new Date(Number(hit.doc.updated_ms)).toISOString())]}
    />
  )
}

/** 144 §7.4: one block of a source or a notebook description — title row or the block's snippet under its heading. */
function BlockRow({ hit, kind }: { hit: BlockHit; kind: 'source' | 'notebook' }) {
  const m = hit.match
  const titled = m.field === 'title'
  return (
    <Lines
      title={titled ? <Snippet text={m.snippet} ranges={m.ranges} /> : hit.title || `Untitled ${kind}`}
      snippet={titled ? null : <Snippet text={m.snippet} ranges={m.ranges} />}
      meta={[kind, hit.heading && hit.heading !== m.snippet ? hit.heading : '', hit.notebooks.length > 1 ? `in ${hit.notebooks.length} notebooks` : '']}
    />
  )
}

function MeetingRow({ hit }: { hit: MeetingSearchHit }) {
  const m = hit.match
  const mt = hit.meeting
  const titled = m.field === 'title'
  const title = mt.title || mt.process_name
  return (
    <Lines
      title={titled ? <Snippet text={m.snippet} ranges={m.ranges} /> : title}
      snippet={titled ? null : <Snippet text={m.snippet} ranges={m.ranges} />}
      meta={[
        'meeting',
        ago(mt.started_at),
        mt.duration_ms === null ? '' : `${Math.round(Number(mt.duration_ms) / 60000)} min`,
        typeof mt.state === 'string' ? mt.state : 'Post',
      ]}
    />
  )
}

