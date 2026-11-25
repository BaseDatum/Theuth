import { Outlet, useNavigate, useParams, useSearch } from '@tanstack/react-router'
import { useVirtualizer } from '@tanstack/react-virtual'
import { createContext, type FocusEvent, type KeyboardEvent, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react'
import { snoozeUntil } from '@/components/hud/model'
import { SEEN_ON_SCREEN, useAttention } from '@/components/hud/queries'
import { SidebarLead } from '@/components/nav/SectionBand'
import { useInsideTile } from '@/components/tiles/tile-context'
import { Bell, ChevronLeft, CircleCheck, type LucideIcon, MessageCircleQuestion, Plus, ShieldCheck } from 'lucide-react'
import { NewChat } from '@/components/chat/NewChatPane'
import { LiveSession } from '@/routes/c.$session'
import { Button } from '@/components/ui/button'
import { useSidebar } from '@/components/ui/sidebar'
import { cornerPad } from '@/lib/sidebar-corner'
import { Skeleton } from '@/components/ui/skeleton'
import { useIsMobile } from '@/hooks/use-mobile'
import { isMacDesktop } from '@/lib/hotkeys'
import { cn } from '@/lib/utils'
import { useOnboarding } from '@/lib/onboarding/state'
import type { AttentionItem } from '@/rpc/bindings/method.roster.output'
import { BulkBar } from './BulkBar'
import { Card, StackCard } from './Card'
import { ATTENTION_EMPTY } from './labels'
import {
  EMPTY_FILTER,
  type Entry,
  type ListFilter,
  type SubtreeFilter,
  type ProjectFilter,
  SORTS,
  type SortKey,
  allApprovals,
  entryHead,
  entryIds,
  entryItems,
  entrySelection,
  focusAfter,
  kindCounts,
  moveFocus,
  nextTick,
  opensFirst,
  paneAfter,
  projectsOf,
  pruneFilter,
  selectEntries,
  selectedIds,
  visibleEntries,
} from './model'
import { useAttentionBulk } from './queries'
import { Toolbar } from './Toolbar'
import { GraphRegion } from './graph/Region'

/** 207 §8.5: sort, kind chips and project, kept across launches (the search is not). */
const VIEW_KEY = 'attention.view'

type View = { filter: ListFilter; sort: SortKey }

function readView(): View {
  try {
    const v: unknown = JSON.parse(localStorage.getItem(VIEW_KEY) ?? 'null')
    if (typeof v !== 'object' || v === null) return { filter: EMPTY_FILTER, sort: 'priority' }
    const o = v as { sort?: unknown; kinds?: unknown; project?: unknown }
    const sort = SORTS.find((s) => s.key === o.sort)?.key ?? 'priority'
    const kinds = new Set(Array.isArray(o.kinds) ? o.kinds.filter((k): k is string => typeof k === 'string') : [])
    const p = o.project as { type?: unknown; name?: unknown } | undefined
    const project: ProjectFilter = p?.type === 'none' ? { type: 'none' } : p?.type === 'project' && typeof p.name === 'string' ? { type: 'project', name: p.name } : { type: 'all' }
    return { filter: { kinds, project, query: '', subtree: null }, sort }
  } catch {
    return { filter: EMPTY_FILTER, sort: 'priority' }
  }
}

function saveView({ filter, sort }: View) {
  try {
    localStorage.setItem(VIEW_KEY, JSON.stringify({ sort, kinds: [...filter.kinds], project: filter.project }))
  } catch {
    // Storage full or blocked: the view lasts this session.
  }
}

/** The list as the rail shows it — `/attention`'s index pane opens its first entry, a screen moves on along it. */
const ViewContext = createContext<{ entries: readonly Entry[] } | null>(null)
export const useAttentionView = () => useContext(ViewContext)

/**
 * The key map stays out of the way of controls: a key pressed on a button, link, field, menu, dialog or popup list is
 * theirs (Enter on a focused Allow clicks Allow; Enter in the Snooze menu picks the preset).
 */
export const CONTROL = 'button,a,input,textarea,select,[contenteditable="true"],[role="menu"],[role="menuitem"],[role="dialog"],[role="combobox"],[role="listbox"],[role="option"],[role="checkbox"]'

/** Cards without a countdown get this coarse clock, so a per-second tick re-renders only the cards that count down. */
const COARSE_MS = 15_000

/** 215: the empty Inbox explains itself: what lands here and why it is empty most of the time. */
const EMPTY_ICON: Record<(typeof ATTENTION_EMPTY.kinds)[number]['id'], LucideIcon> = { approval: ShieldCheck, ask: MessageCircleQuestion, finished: CircleCheck, alert: Bell }
function Empty() {
  return (
    <div className="m-3 rounded-xl border border-border-subtle bg-elevated/60 px-4 py-4">
      <p className="text-sm font-medium">{ATTENTION_EMPTY.title}</p>
      <p className="mt-1 text-[13px] leading-relaxed text-fg-secondary">{ATTENTION_EMPTY.intro}</p>
      <ul className="mt-3 flex flex-col gap-2">
        {ATTENTION_EMPTY.kinds.map(({ id, label, text }) => {
          const Icon = EMPTY_ICON[id]
          return (
            <li key={id} className="flex items-start gap-2.5 text-[13px] leading-snug">
              <Icon aria-hidden className="mt-px size-4 shrink-0 text-[var(--fg-accent)]" />
              <span className="text-fg-tertiary">
                <span className="font-medium text-fg-secondary">{label}</span> · {text}
              </span>
            </li>
          )
        })}
      </ul>
    </div>
  )
}

/**
 * `/attention` (101 D6, 207 §6): the 52 px header, then a 440 px rail with the toolbar and ONE flat, virtualised list of
 * cards in the engine's order (or the chosen sort) beside the selected item's screen (`<Outlet/>`). Under `md` one pane
 * at a time: the list at `/attention`, the screen at `/attention/$id` (which carries its own header with the back chevron).
 * Keys on the list (not on a control or while typing): J/K move · Enter opens · A/D allow/deny (a collapsed stack opens
 * first) · S snooze · E dismiss · X select · Shift+X range · / search · Esc clears the search, then the selection.
 */
/** A slim bar over a pinned chat or draft: back to the item the pane was opened from. */
function PaneBar({ onBack }: { onBack: () => void }) {
  return (
    <div className="flex h-9 shrink-0 items-center border-b border-border-subtle px-2">
      <Button size="xs" variant="ghost" onClick={onBack}>
        <ChevronLeft />
        Inbox
      </Button>
    </div>
  )
}

export function Page() {
  useEffect(() => useOnboarding.getState().markVisited('inbox'), [])
  const sidebar = useSidebar()
  const mac = isMacDesktop()
  const insideTile = useInsideTile()
  const attention = useAttention()
  const bulk = useAttentionBulk()
  const navigate = useNavigate()
  const { id: active } = useParams({ strict: false })
  const search = useSearch({ strict: false }) as { chat?: string; new?: true }
  const items = attention.data
  const [view, setViewState] = useState(readView)
  const { filter, sort } = view
  const setView = (next: View) => {
    setViewState(next)
    saveView(next)
  }
  /** Item ids, snapshotted when selected (review 12). */
  const [selection, setSelection] = useState<ReadonlySet<string>>(() => new Set())
  const [anchor, setAnchor] = useState<string | null>(null)
  const [focus, setFocus] = useState<string | null>(null)
  /** Stacks whose members are shown (a keyboard A / D opens a stack before it acts on it). */
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(() => new Set())
  const [now, setNow] = useState(Date.now)
  const entries = useMemo(() => visibleEntries(items ?? [], filter, sort, now), [items, filter, sort, now])
  const keys = useMemo(() => entries.map((e) => e.key), [entries])
  const shownIds = useMemo(() => entries.flatMap(entryIds), [entries])
  const live = useMemo(() => (items ?? []).filter((i) => typeof i.expires_ms !== 'number' || i.expires_ms > now), [items, now])
  const kinds = useMemo(() => kindCounts(live), [live])
  const projects = useMemo(() => projectsOf(live), [live])
  const filtered = filter.kinds.size > 0 || filter.project.type !== 'all' || filter.query !== '' || filter.subtree !== null
  const searchRef = useRef<HTMLInputElement>(null)
  const scrollRef = useRef<HTMLDivElement>(null)

  // Review 14: one timer — every second while a visible card counts down, else at the next expiry (at most 30 s).
  useEffect(() => {
    const t = setTimeout(() => setNow(Date.now()), nextTick(entries.flatMap(entryItems), now))
    return () => clearTimeout(t)
  }, [entries, now])
  // Review 11: a remembered kind or project that is no longer listed would leave "Nothing matches" with no chip to undo.
  useEffect(() => {
    if (!items) return
    const pruned = pruneFilter(filter, kinds.map((k) => k.kind), projects.map((p) => p.name))
    if (pruned !== filter) setView({ filter: pruned, sort })
  })
  // Items that leave the list leave the selection.
  useEffect(() => {
    const present = new Set(shownIds)
    setSelection((prev) => {
      const kept = [...prev].filter((id) => present.has(id))
      return kept.length === prev.size ? prev : new Set(kept)
    })
  }, [shownIds])
  // The open item left the list — dismissed, snoozed or resolved from its card, the bulk bar or the keyboard, answered
  // elsewhere, expired: the pane moves on as the screen's own footer would (the next item, else the one before, else
  // the index). Phones show one pane at a time and keep the screen up. A show marked seen by being on screen stays up
  // until the user moves (104 A2), and an item merely filtered out of view stays open.
  const mobile = useIsMobile()
  const prevEntries = useRef(entries)
  useEffect(() => {
    const prev = prevEntries.current
    prevEntries.current = entries
    if (!active || !items || mobile || SEEN_ON_SCREEN.has(active)) return
    if (items.some((i) => i.id === active) || !prev.some((e) => entryIds(e).includes(active))) return
    const next = paneAfter(prev, entries, active)
    void (next ? navigate({ to: '/attention/$id', params: { id: next }, search: true }) : navigate({ to: '/attention', search: true }))
  }, [entries, items, active, mobile, navigate])

  const v = useVirtualizer({
    count: entries.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => 112,
    overscan: 8,
    getItemKey: (i) => entries[i]?.key ?? i,
  })

  // Review 2: the focused card left (resolved, filtered out) — focus the one now in its place and give the scroller DOM
  // focus back when it fell to the body, so the key map keeps working.
  const prevKeys = useRef(keys)
  useEffect(() => {
    const prev = prevKeys.current
    prevKeys.current = keys
    if (focus === null || keys.includes(focus)) return
    const next = focusAfter(prev, keys, focus)
    setFocus(next)
    const i = next === null ? -1 : keys.indexOf(next)
    if (i >= 0) v.scrollToIndex(i, { align: 'auto' })
    const el = document.activeElement
    if (!el || el === document.body || !el.isConnected) scrollRef.current?.focus({ preventScroll: true })
  }, [keys, focus, v])

  const select = (key: string, mode: 'toggle' | 'range') => {
    setSelection((prev) => selectEntries(prev, entries, anchor, key, mode))
    setAnchor(key)
    setFocus(key)
  }
  /** After a bulk call: the selection keeps only what failed, so it can be retried. */
  const keepFailed = (failed: readonly string[]) => {
    setSelection(new Set(failed))
    if (failed.length === 0) setAnchor(null)
  }
  const clearSelection = () => keepFailed([])
  const open = (key: string) => {
    const e = entries.find((x) => x.key === key)
    if (!e) return
    setFocus(key)
    // Tapping an item returns the pane to that item (a pinned chat or draft is left behind deliberately).
    void navigate({ to: '/attention/$id', params: { id: entryHead(e).id }, search: {} })
  }
  const expand = (key: string, on: boolean) =>
    setExpanded((prev) => {
      const next = new Set(prev)
      if (on) next.add(key)
      else next.delete(key)
      return next
    })
  /** 208: "Show in graph" — the region expands and centres on the nonce. */
  const [graphFocus, setGraphFocus] = useState<{ session: string; nonce: number } | null>(null)
  const showInGraph = (session: string) => setGraphFocus((f) => ({ session, nonce: (f?.nonce ?? 0) + 1 }))
  const setSubtree = (subtree: SubtreeFilter | null) => setView({ filter: { ...filter, subtree }, sort })
  // The detail pane's three modes: the selected item, a live session (graph tap), a new-session draft.
  const pane = search.new ? 'new' : search.chat ? 'chat' : 'attention'
  const paneSearch = (s: { chat: string } | { new: true } | Record<string, never>) => void navigate({ to: active ? '/attention/$id' : '/attention', params: active ? { id: active } : undefined, search: s })
  const openChat = (session: string) => paneSearch({ chat: session })
  const openNew = () => paneSearch({ new: true })
  const closePane = () => paneSearch({})
  // Stable callbacks for the memoised cards (review 14): they read the latest closures through a ref.
  const latest = useRef({ select, open, expand, showInGraph, setSubtree })
  latest.current = { select, open, expand, showInGraph, setSubtree }
  const onSelect = useCallback((mode: 'toggle' | 'range', key: string) => latest.current.select(key, mode), [])
  const onOpen = useCallback((key: string) => latest.current.open(key), [])
  const onExpand = useCallback((key: string, on: boolean) => latest.current.expand(key, on), [])
  const onShowInGraph = useCallback((session: string) => latest.current.showInGraph(session), [])
  const onSubtree = useCallback((f: SubtreeFilter | null) => latest.current.setSubtree(f), [])

  const focusTo = (key: string | null) => {
    setFocus(key)
    const i = key === null ? -1 : keys.indexOf(key)
    // Instant, not smooth: J/K repeat fast, measured rows make smooth scrolling drift, and it honours reduced motion.
    if (i >= 0) v.scrollToIndex(i, { align: 'auto' })
  }
  const focused = entries.find((e) => e.key === focus)
  // 208: the graph highlights the focused entry's session, else the open item's.
  const openItem = active ? items?.find((i) => i.id === active) : undefined
  const graphHighlight = (focused ? entryHead(focused).session : openItem?.session) ?? null
  const approvalsOf = (xs: readonly AttentionItem[]) => xs.flatMap((i) => (i.kind === 'approval' && i.approval ? [i.approval] : []))
  const selected = selectedIds(entries, selection)
  const selectedItems = entries.flatMap(entryItems).filter((i) => selection.has(i.id))

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.defaultPrevented || e.metaKey || e.ctrlKey || e.altKey) return
    const target = e.target as Element
    if (target !== e.currentTarget && target.closest(CONTROL)) return
    // Review 7: one bulk call at a time — keys wait while one runs.
    if (bulk.pending) return
    const key = e.key.length === 1 ? e.key.toLowerCase() : e.key
    const handled = () => e.preventDefault()
    switch (key) {
      case 'j':
      case 'k':
        handled()
        focusTo(moveFocus(keys, focus, key === 'j' ? 1 : -1))
        return
      case 'Enter':
        if (focused) {
          handled()
          open(focused.key)
        }
        return
      case 'a':
      case 'd': {
        if (!focused) return
        const approvals = approvalsOf(entryItems(focused))
        if (approvals.length === 0) return
        handled()
        // Review 3: a collapsed stack opens first, so N commands are never decided unseen.
        if (opensFirst(focused, expanded, false)) expand(focused.key, true)
        else void bulk.resolve(approvals, key === 'a' ? 'allow' : 'deny')
        return
      }
      case 's':
      case 'e': {
        const ids = selection.size > 0 ? selected : focused ? entryIds(focused) : []
        if (ids.length === 0) return
        handled()
        // The same guard for snooze and dismiss on a focused stack (a selection was chosen on purpose).
        if (focused && opensFirst(focused, expanded, selection.size > 0)) {
          expand(focused.key, true)
          return
        }
        const call = key === 's' ? bulk.snooze(ids, snoozeUntil('tomorrow')) : bulk.dismiss(ids)
        if (selection.size > 0) void call.then((outs) => keepFailed(outs.filter((o) => o.resolution === 'failed').map((o) => o.id)))
        return
      }
      case 'x':
        if (focus !== null && keys.includes(focus)) {
          handled()
          select(focus, e.shiftKey ? 'range' : 'toggle')
        }
        return
      case '/':
        handled()
        searchRef.current?.focus()
        return
      case 'Escape':
        // Review 4: Shift+Escape is the app's "focus the composer".
        if (e.shiftKey) return
        if (filter.query) {
          handled()
          setView({ filter: { ...filter, query: '' }, sort })
        } else if (selection.size > 0) {
          handled()
          clearSelection()
        }
        return
    }
  }
  // Review 1: focusing anything inside a card (Tab to its Allow) makes that card the focused entry.
  const onFocus = (e: FocusEvent<HTMLDivElement>) => {
    const k = (e.target as Element).closest('[data-entry-key]')?.getAttribute('data-entry-key')
    if (k) setFocus(k)
    else if (focus === null) setFocus(keys[0] ?? null)
  }

  return (
    <ViewContext.Provider value={{ entries }}>
      <article className="flex min-h-0 min-w-0 flex-1 flex-col">
        <header
          data-tauri-drag-region={(!insideTile && mac) || undefined}
          className={cn(
            'shrink-0 items-center gap-1 border-b border-border-subtle px-3',
            insideTile ? 'h-10' : 'h-[52px]',
            !insideTile && mac && cornerPad(sidebar),
            active ? 'hidden md:flex' : 'flex',
          )}
        >
          {!insideTile && <SidebarLead />}
          <h2 className="min-w-0 flex-1 truncate text-sm font-semibold">
            Inbox {items && <span className="font-normal text-fg-tertiary tabular-nums">{live.length}</span>}
          </h2>
          <Button size="xs" variant="outline" onClick={openNew} title="Start a new session in the detail pane">
            <Plus />
            New session
          </Button>
        </header>
        <div className={cn(active ? 'hidden md:contents' : 'contents')}>
          <GraphRegion items={items} live={live.length} highlight={graphHighlight} focus={graphFocus} subtree={filter.subtree?.session ?? null} onSubtree={onSubtree} onOpenSession={openChat} />
        </div>
        <div className="flex min-h-0 flex-1 overflow-hidden">
          <aside data-tour="inbox-list" className={cn('w-full shrink-0 flex-col border-r border-border-subtle md:flex md:w-[440px]', active || pane !== 'attention' ? 'hidden' : 'flex')}>
            {attention.isPending ? (
              <div className="flex flex-col gap-3 p-3">
                <Skeleton className="h-24 w-full" />
                <Skeleton className="h-24 w-full" />
                <Skeleton className="h-24 w-full" />
              </div>
            ) : attention.isError ? (
              <p className="p-3 text-sm text-danger">{attention.error.message}</p>
            ) : live.length === 0 ? (
              <Empty />
            ) : (
              <>
                <Toolbar filter={filter} sort={sort} kinds={kinds} projects={projects} searchRef={searchRef} onFilter={(f) => setView({ filter: f, sort })} onSort={(s) => setView({ filter, sort: s })} />
                <div className="flex h-8 shrink-0 items-center gap-2 border-b border-border-subtle px-3 text-xs text-fg-tertiary tabular-nums">
                  <span className="mr-auto">
                    {live.length} {live.length === 1 ? 'item' : 'items'}
                    {shownIds.length !== live.length && ` · ${shownIds.length} shown`}
                  </span>
                  {filtered && (
                    <Button size="xs" variant="ghost" onClick={() => setView({ filter: EMPTY_FILTER, sort })}>
                      Clear filters
                    </Button>
                  )}
                  {entries.length > 0 && (
                    <Button size="xs" variant="ghost" onClick={() => setSelection(new Set(shownIds))}>
                      Select all {shownIds.length}
                    </Button>
                  )}
                </div>
                {entries.length === 0 ? (
                  <div className="m-3 flex flex-col items-start gap-2 rounded-lg border border-dashed border-border-subtle px-3 py-3 text-sm text-fg-tertiary">
                    <p>Nothing matches.</p>
                    <Button size="xs" variant="outline" onClick={() => setView({ filter: EMPTY_FILTER, sort })}>
                      Clear filters
                    </Button>
                  </div>
                ) : (
                  // The key map lives on the scroller, so J/K work wherever inside the list focus is.
                  <div
                    ref={scrollRef}
                    tabIndex={0}
                    role="region"
                    aria-label="Inbox items"
                    aria-keyshortcuts="J K Enter A D S E X Shift+X / Escape"
                    onKeyDown={onKeyDown}
                    onFocus={onFocus}
                    className="min-h-0 flex-1 overflow-y-auto outline-none focus-visible:ring-1 focus-visible:ring-border-strong focus-visible:ring-inset"
                  >
                    <ul className="relative w-full" style={{ height: v.getTotalSize() }}>
                      {v.getVirtualItems().map((row) => {
                        const entry = entries[row.index]
                        if (!entry) return null
                        const counts = entryItems(entry).some((i) => typeof i.deadline_ms === 'number')
                        const sel = entrySelection(entry, selection)
                        const common = {
                          'data-index': row.index,
                          ref: v.measureElement,
                          className: 'absolute top-0 left-0 w-full',
                          style: { transform: `translateY(${row.start}px)` },
                          entryKey: entry.key,
                          now: counts ? now : Math.floor(now / COARSE_MS) * COARSE_MS,
                          focused: entry.key === focus,
                          selected: sel === 'all' ? true : sel === 'some' ? ('some' as const) : false,
                          selecting: selection.size > 0,
                          active: entryItems(entry).some((i) => i.id === active),
                          onSelect,
                          onOpen,
                          onShowInGraph,
                        }
                        return entry.type === 'item' ? (
                          <Card key={entry.key} item={entry.item} {...common} />
                        ) : (
                          <StackCard key={entry.key} items={entry.items} expanded={expanded.has(entry.key)} onExpand={onExpand} {...common} />
                        )
                      })}
                    </ul>
                  </div>
                )}
                {selected.length > 0 && (
                  <BulkBar
                    count={selected.length}
                    approvals={allApprovals(entries, selection)}
                    pending={bulk.pending}
                    onDismiss={() => void bulk.dismiss(selected).then((outs) => keepFailed(outs.filter((o) => o.resolution === 'failed').map((o) => o.id)))}
                    onSnooze={(until) => void bulk.snooze(selected, until).then((outs) => keepFailed(outs.filter((o) => o.resolution === 'failed').map((o) => o.id)))}
                    onResolve={(decision) => void bulk.resolve(approvalsOf(selectedItems), decision).then((outs) => keepFailed(outs.filter((o) => o.result === 'failed').map((o) => o.id)))}
                    onClear={clearSelection}
                  />
                )}
              </>
            )}
          </aside>
          <main className={cn('min-h-0 min-w-0 flex-1 flex-col md:flex', active || pane !== 'attention' ? 'flex' : 'hidden')}>
            {pane === 'chat' && search.chat ? (
              <div className="flex min-h-0 flex-1 flex-col">
                <PaneBar onBack={closePane} />
                <LiveSession key={search.chat} session={search.chat} first={false} />
              </div>
            ) : pane === 'new' ? (
              <div className="flex min-h-0 flex-1 flex-col overflow-y-auto">
                <PaneBar onBack={closePane} />
                <NewChat first={false} onCreated={openChat} />
              </div>
            ) : (
              <Outlet />
            )}
          </main>
        </div>
      </article>
    </ViewContext.Provider>
  )
}
