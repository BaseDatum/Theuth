import { useQueries, useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { Command as CommandPrimitive } from 'cmdk'
import { Check, FileText, History, MessageSquare, RefreshCw, Search, X } from 'lucide-react'
import { type ReactNode, useEffect, useRef, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Command, CommandGroup, CommandItem, CommandList } from '@/components/ui/command'
import { Dialog, DialogClose, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { queryKeys, useApi } from '@/lib/rpc'
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
import { cn } from '@/lib/utils'
import type { ArtifactSearchHit } from '@/rpc/bindings/ArtifactSearchHit'
import type { ConceptSearchHit } from '@/rpc/bindings/ConceptSearchHit'
import type { HistorySearchHit } from '@/rpc/bindings/HistorySearchHit'
import type { MatchRange } from '@/rpc/bindings/MatchRange'
import type { SearchCategory } from '@/rpc/bindings/SearchCategory'
import type { SearchGroup } from '@/rpc/bindings/SearchGroup'
import type { SearchHit } from '@/rpc/bindings/SearchHit'
import type { SearchNotice } from '@/rpc/bindings/SearchNotice'
import type { SearchPage } from '@/rpc/bindings/SearchPage'
import type { SearchResponse } from '@/rpc/bindings/SearchResponse'
import { ago } from './AgentTree'

const DEBOUNCE_MS = 150
const INDEXING_POLL_MS = 500
const RECENT = 10

const ICON: Record<SearchCategory, typeof Search> = {
  sessions: MessageSquare,
  artifacts: FileText,
  knowledge: Search,
  knowledge_history: History,
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

/** One activatable row: cmdk value/React key is the stable source identity (49 §2.3), never a row index. */
type Item = { key: string; dest: SearchDestination; node: ReactNode }

function items(g: SearchGroup): Item[] {
  switch (g.category) {
    case 'sessions':
      return g.page.hits.map((h) => ({
        key: `sessions:${h.session}:${h.seq ?? 'title'}`,
        dest: { category: 'sessions', session: h.session, seq: h.seq },
        node: <SessionRow hit={h} />,
      }))
    case 'artifacts':
      return g.page.hits.map((h) => ({
        key: `artifacts:${h.artifact.call_id}`,
        dest: { category: 'artifacts', callId: h.artifact.call_id },
        node: <ArtifactRow hit={h} />,
      }))
    case 'knowledge':
      return g.page.hits.map((h) => ({
        key: `knowledge:${h.meta.id}`,
        dest: { category: 'knowledge', id: h.meta.id },
        node: <ConceptRow hit={h} />,
      }))
    case 'knowledge_history':
      return g.page.hits.map((h) => ({
        key: `knowledge_history:${h.commit}:${h.id}`,
        dest: { category: 'knowledge_history', id: h.id, commit: h.commit, snapshot: h.snapshot_commit },
        node: <HistoryRow hit={h} />,
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
  const api = useApi()
  const qc = useQueryClient()
  const navigate = useNavigate()
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
  const inputRef = useRef<HTMLInputElement>(null)
  const listRef = useRef<HTMLDivElement>(null)
  const opener = useRef<HTMLElement | null>(null)
  const activated = useRef(false)
  const wasOpen = useRef(false)
  const pendingScroll = useRef<number | null>(null)
  const refreshGen = useRef(0)
  /** Dialog-open generation (49 §5.3): every request of this open is keyed by it, so a late success or error from a previous open lands under a key nothing observes. */
  const opens = useRef(0)
  const [gen, setGen] = useState(0)

  const cats = override ?? remembered
  const query = typed.trim()
  /** UTF-16 length bounds the scalar count; only an over-long string pays for the exact count. */
  const tooLong = query.length > MAX_QUERY_CHARS && Array.from(query).length > MAX_QUERY_CHARS
  /** Edits not yet requested: rows on screen belong to the previous query and must not open. */
  const pending = query !== debounced
  const searching = open && debounced !== '' && cats.length > 0 && !tooLong

  useEffect(() => {
    if (composing || query === debounced) return
    const t = setTimeout(() => {
      setDebounced(query)
      setLimits(DEFAULT_LIMITS)
    }, DEBOUNCE_MS)
    return () => clearTimeout(t)
  }, [query, debounced, composing])

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
    queryFn: () => api.search(debounced, cats, LIMITS[0]),
    enabled: searching,
  })
  const moreCats = cats.filter((c) => limits[c] > LIMITS[0])
  const more = useQueries({
    queries: moreCats.map((c) => ({
      queryKey: queryKeys.search(debounced, [c], limits[c], gen),
      queryFn: () => api.search(debounced, [c], limits[c]),
      enabled: searching,
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
      const fresh = await api.search(debounced, building, LIMITS[0])
      qc.setQueryData<SearchResponse>(baseKey, (old) => old && { groups: patch(old.groups, fresh.groups) })
      return fresh
    },
    enabled: searching && building.length > 0,
    refetchInterval: INDEXING_POLL_MS,
    refetchIntervalInBackground: false,
  })
  const recent = useQuery({
    queryKey: queryKeys.sessions,
    queryFn: () => api.list_sessions(),
    enabled: open && debounced === '' && cats.includes('sessions'),
  })

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
        <div className="flex h-11 shrink-0 items-center justify-between border-b pr-1 pl-4">
          <DialogTitle className="text-sm font-medium">Search</DialogTitle>
          <DialogDescription className="sr-only">
            Search sessions, artifacts, knowledge and knowledge history on this harness. Arrow keys move
            between results; Enter opens the selected result.
          </DialogDescription>
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
              value={typed}
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
          </div>
          {/* Source controls: keydown does not reach cmdk's root, so chip Enter/Space/arrows stay chip-local. */}
          <div
            className="flex shrink-0 flex-wrap items-center gap-2 border-b px-3 py-2"
            onKeyDown={(e) => e.stopPropagation()}
          >
            <span id="search-in" className="text-xs text-fg-tertiary">
              Search in
            </span>
            <button
              type="button"
              aria-pressed={allSelected}
              onClick={() => {
                if (!allSelected) select([...CATEGORIES])
              }}
              className={chip(allSelected)}
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
              className="flex-wrap"
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
                          {s.name ? `↳ ${s.name}` : s.title || s.id.slice(0, 8)}
                        </span>
                        <span className="text-xs text-fg-tertiary">{ago(s.updated_at)}</span>
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
                    />
                  ) : (
                    <Section key={category} category={category} state="No answer for this category." />
                  ),
                )}
              </>
            )}
          </CommandList>
        </Command>
        <div className="flex shrink-0 flex-wrap items-center gap-x-3 gap-y-1 border-t px-3 py-2 text-xs text-fg-tertiary">
          <Button
            variant="ghost"
            size="sm"
            className="pointer-coarse:min-h-11"
            disabled={refreshing !== null || cats.length === 0}
            aria-busy={refreshing !== null}
            aria-label={refreshing ? 'Refreshing search sources' : 'Refresh search sources'}
            onClick={() => void refresh(cats)}
          >
            <RefreshCw className={cn('size-3.5', refreshing && 'motion-safe:animate-spin')} aria-hidden />
            {refreshing ? 'Refreshing…' : 'Refresh'}
          </Button>
          {refreshError && (
            <span role="alert" className="text-destructive">
              Refresh failed: {refreshError}
            </span>
          )}
          <span className="ml-auto hidden md:inline">Arrow keys · Enter open</span>
          <details className="w-full md:w-auto">
            <summary className="cursor-default select-none pointer-coarse:min-h-11 pointer-coarse:py-3">Search help</summary>
            <div className="mt-1 space-y-1 text-fg-secondary">
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
          </details>
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

/** Heading suffix and live-region wording for one page. */
function status(page: SearchPage<unknown>): string {
  switch (page.state) {
    case 'ready':
      return page.hits.length === 0
        ? 'no matches'
        : page.has_more
          ? `${page.hits.length}+ matches`
          : `${page.hits.length} shown`
    case 'indexing':
      return 'indexing…'
    case 'unavailable':
      return 'unavailable'
    case 'error':
      return 'error'
  }
}

function Notice({ children, tone }: { children: ReactNode; tone?: 'error' }) {
  return (
    <div
      role={tone === 'error' ? 'alert' : undefined}
      className={cn('px-3 py-6 text-center text-sm', tone === 'error' ? 'text-destructive' : 'text-fg-secondary')}
    >
      {children}
    </div>
  )
}

function Notices({ notices }: { notices: SearchNotice[] }) {
  if (notices.length === 0) return null
  return (
    <ul className="px-2 pb-2 text-xs text-fg-tertiary">
      {notices.map((n, i) => (
        <li key={i}>
          {n.message}
          {n.affected !== null && ` (${n.affected})`}
        </li>
      ))}
    </ul>
  )
}

/** A category heading with a state line and no rows: unavailable/error/indexing, or a missing group. */
function Section({
  category,
  state,
  children,
}: {
  category: SearchCategory
  state: string
  children?: ReactNode
}) {
  return (
    <div className="px-1 pt-2">
      <div className="px-2 py-1.5 text-xs font-medium text-muted-foreground">
        {CATEGORY_LABEL[category]} · <span id={`search-state-${category}`}>{state}</span>
      </div>
      {children}
    </div>
  )
}

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
          className="min-h-11 items-start gap-3 py-2"
        >
          <Icon className="mt-0.5" aria-hidden />
          {it.node}
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

function SessionRow({ hit }: { hit: SearchHit }) {
  const title = hit.kind === 'title' ? <Snippet text={hit.snippet} ranges={hit.ranges} /> : hit.title || hit.session.slice(0, 8)
  return (
    <Lines
      title={title}
      snippet={hit.kind === 'message' ? <Snippet text={hit.snippet} ranges={hit.ranges} /> : null}
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
        a.session.slice(0, 8),
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

/** `ranges` are `[start, end)` char offsets (not UTF-16 units), so slice the code-point array. */
function Snippet({ text, ranges }: { text: string; ranges: MatchRange[] }) {
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
