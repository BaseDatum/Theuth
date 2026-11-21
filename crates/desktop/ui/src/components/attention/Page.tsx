import { Outlet, useNavigate, useParams } from '@tanstack/react-router'
import { useVirtualizer } from '@tanstack/react-virtual'
import { createContext, type KeyboardEvent, useContext, useEffect, useMemo, useRef, useState } from 'react'
import { snoozeUntil } from '@/components/hud/model'
import { useAttention } from '@/components/hud/queries'
import { SidebarLead } from '@/components/nav/SectionBand'
import { Button } from '@/components/ui/button'
import { useSidebar } from '@/components/ui/sidebar'
import { Skeleton } from '@/components/ui/skeleton'
import { isMacDesktop } from '@/lib/hotkeys'
import { nextSelection } from '@/lib/selection'
import { cn } from '@/lib/utils'
import type { AttentionItem } from '@/rpc/bindings/method.roster.output'
import { BulkBar } from './BulkBar'
import { Card, StackCard } from './Card'
import { ATTENTION_EMPTY } from './labels'
import {
  EMPTY_FILTER,
  type Entry,
  type ListFilter,
  type ProjectFilter,
  SORTS,
  type SortKey,
  allApprovals,
  entryHead,
  entryItems,
  kindCounts,
  moveFocus,
  projectsOf,
  selectedIds,
  visibleEntries,
} from './model'
import { useAttentionBulk } from './queries'
import { Toolbar } from './Toolbar'

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
    return { filter: { kinds, project, query: '' }, sort }
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

/** The list as the rail shows it — `/attention`'s index pane opens its first entry. */
const ViewContext = createContext<{ entries: readonly Entry[] } | null>(null)
export const useAttentionView = () => useContext(ViewContext)

/** A clock for countdowns and expiry: every second while something counts down, else every 30 s. */
function useNow(fast: boolean): number {
  const [now, setNow] = useState(Date.now)
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), fast ? 1000 : 30_000)
    return () => clearInterval(t)
  }, [fast])
  return now
}

/** The key map ignores keys typed into a field. */
const typing = (t: EventTarget): boolean => t instanceof HTMLElement && (t.isContentEditable || t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.tagName === 'SELECT')

/**
 * `/attention` (101 D6, 207 §6): the 52 px header, then a 440 px rail with the toolbar and ONE flat, virtualised list of
 * cards in the engine's order (or the chosen sort) beside the selected item's screen (`<Outlet/>`). Under `md` one pane
 * at a time: the list at `/attention`, the screen at `/attention/$id` (which carries its own header with the back chevron).
 * Keys on the list (not while typing): J/K move · Enter opens · A/D allow/deny · S snooze · E dismiss · X select ·
 * Shift+X range · / search · Esc clears the search, then the selection.
 */
export function Page() {
  const sidebar = useSidebar()
  const mac = isMacDesktop()
  const attention = useAttention()
  const bulk = useAttentionBulk()
  const navigate = useNavigate()
  const { id: active } = useParams({ strict: false })
  const items = attention.data
  const [view, setViewState] = useState(readView)
  const { filter, sort } = view
  const setView = (next: View) => {
    setViewState(next)
    saveView(next)
  }
  const [selection, setSelection] = useState<ReadonlySet<string>>(() => new Set())
  const [anchor, setAnchor] = useState<string | null>(null)
  const [focus, setFocus] = useState<string | null>(null)
  const [ticking, setTicking] = useState(false)
  const now = useNow(ticking)
  const entries = useMemo(() => visibleEntries(items ?? [], filter, sort, now), [items, filter, sort, now])
  const keys = useMemo(() => entries.map((e) => e.key), [entries])
  const shown = useMemo(() => entries.reduce((n, e) => n + entryItems(e).length, 0), [entries])
  const live = useMemo(() => (items ?? []).filter((i) => typeof i.expires_ms !== 'number' || i.expires_ms > now), [items, now])
  const kinds = useMemo(() => kindCounts(live), [live])
  const projects = useMemo(() => projectsOf(live), [live])
  const filtered = filter.kinds.size > 0 || filter.project.type !== 'all' || filter.query !== ''
  const searchRef = useRef<HTMLInputElement>(null)
  const scrollRef = useRef<HTMLDivElement>(null)

  useEffect(() => setTicking(entries.some((e) => entryItems(e).some((i) => typeof i.deadline_ms === 'number'))), [entries])
  // Entries that leave the list leave the selection.
  useEffect(() => {
    const present = new Set(keys)
    setSelection((prev) => {
      const kept = [...prev].filter((k) => present.has(k))
      return kept.length === prev.size ? prev : new Set(kept)
    })
  }, [keys])

  const v = useVirtualizer({
    count: entries.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => 112,
    overscan: 8,
    getItemKey: (i) => entries[i]?.key ?? i,
  })

  const select = (key: string, mode: 'toggle' | 'range') => {
    setSelection((prev) => nextSelection(prev, keys, anchor, key, mode))
    setAnchor(key)
    setFocus(key)
  }
  const clearSelection = () => {
    setSelection(new Set())
    setAnchor(null)
  }
  const open = (e: Entry) => {
    setFocus(e.key)
    void navigate({ to: '/attention/$id', params: { id: entryHead(e).id }, search: true })
  }
  const focusTo = (key: string | null) => {
    setFocus(key)
    const i = key === null ? -1 : keys.indexOf(key)
    // Instant, not smooth: J/K repeat fast, measured rows make smooth scrolling drift, and it honours reduced motion.
    if (i >= 0) v.scrollToIndex(i, { align: 'auto' })
  }
  const focused = entries.find((e) => e.key === focus)
  /** The selection's ids, else the focused entry's. */
  const targetIds = () => (selection.size > 0 ? selectedIds(entries, selection) : focused ? entryItems(focused).map((i) => i.id) : [])
  const approvalsOf = (xs: readonly AttentionItem[]) => xs.flatMap((i) => (i.kind === 'approval' && i.approval ? [i.approval] : []))
  const selectedItems = entries.filter((e) => selection.has(e.key)).flatMap(entryItems)

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (typing(e.target) || e.metaKey || e.ctrlKey || e.altKey) return
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
          open(focused)
        }
        return
      case 'a':
      case 'd': {
        const approvals = focused ? approvalsOf(entryItems(focused)) : []
        if (approvals.length > 0) {
          handled()
          void bulk.resolve(approvals, key === 'a' ? 'allow' : 'deny')
        }
        return
      }
      case 's':
      case 'e': {
        const ids = targetIds()
        if (ids.length === 0) return
        handled()
        void (key === 's' ? bulk.snooze(ids, snoozeUntil('tomorrow')) : bulk.dismiss(ids)).then(clearSelection)
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

  return (
    <ViewContext.Provider value={{ entries }}>
      <article className="flex min-h-0 min-w-0 flex-1 flex-col">
        <header
          data-tauri-drag-region={mac || undefined}
          className={cn('h-[52px] shrink-0 items-center gap-1 border-b border-border-subtle px-3', !sidebar.open && mac && 'pl-20', active ? 'hidden md:flex' : 'flex')}
        >
          <SidebarLead />
          <h2 className="min-w-0 flex-1 truncate text-sm font-semibold">
            Attention {items && <span className="font-normal text-fg-tertiary tabular-nums">{live.length}</span>}
          </h2>
        </header>
        <div className="flex min-h-0 flex-1 overflow-hidden">
          <aside className={cn('w-full shrink-0 flex-col border-r border-border-subtle md:flex md:w-[440px]', active ? 'hidden' : 'flex')}>
            {attention.isPending ? (
              <div className="flex flex-col gap-3 p-3">
                <Skeleton className="h-24 w-full" />
                <Skeleton className="h-24 w-full" />
                <Skeleton className="h-24 w-full" />
              </div>
            ) : attention.isError ? (
              <p className="p-3 text-sm text-danger">{attention.error.message}</p>
            ) : live.length === 0 ? (
              <p className="m-3 rounded-lg border border-dashed border-border-subtle px-3 py-3 text-sm text-fg-tertiary">{ATTENTION_EMPTY}</p>
            ) : (
              <>
                <Toolbar filter={filter} sort={sort} kinds={kinds} projects={projects} searchRef={searchRef} onFilter={(f) => setView({ filter: f, sort })} onSort={(s) => setView({ filter, sort: s })} />
                <div className="flex h-8 shrink-0 items-center gap-2 border-b border-border-subtle px-3 text-xs text-fg-tertiary tabular-nums">
                  <span className="mr-auto">
                    {live.length} {live.length === 1 ? 'item' : 'items'}
                    {shown !== live.length && ` · ${shown} shown`}
                  </span>
                  {filtered && (
                    <Button size="xs" variant="ghost" onClick={() => setView({ filter: EMPTY_FILTER, sort })}>
                      Clear filters
                    </Button>
                  )}
                  {entries.length > 0 && (
                    <Button size="xs" variant="ghost" onClick={() => setSelection(new Set(keys))}>
                      Select all {shown}
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
                    aria-label="Attention items"
                    aria-keyshortcuts="J K Enter A D S E X Shift+X / Escape"
                    onKeyDown={onKeyDown}
                    onFocus={() => focus === null && setFocus(keys[0] ?? null)}
                    className="min-h-0 flex-1 overflow-y-auto outline-none"
                  >
                    <ul className="relative w-full" style={{ height: v.getTotalSize() }}>
                      {v.getVirtualItems().map((row) => {
                        const entry = entries[row.index]
                        if (!entry) return null
                        const common = {
                          'data-index': row.index,
                          ref: v.measureElement,
                          className: 'absolute top-0 left-0 w-full',
                          style: { transform: `translateY(${row.start}px)` },
                          now,
                          focused: entry.key === focus,
                          selected: selection.has(entry.key),
                          selecting: selection.size > 0,
                          active: entryItems(entry).some((i) => i.id === active),
                          onSelect: (mode: 'toggle' | 'range') => select(entry.key, mode),
                          onOpen: () => open(entry),
                        }
                        return entry.type === 'item' ? <Card key={entry.key} item={entry.item} {...common} /> : <StackCard key={entry.key} items={entry.items} {...common} />
                      })}
                    </ul>
                  </div>
                )}
                {selection.size > 0 && (
                  <BulkBar
                    count={selectedItems.length}
                    approvals={allApprovals(entries, selection)}
                    pending={bulk.pending}
                    onDismiss={() => void bulk.dismiss(selectedIds(entries, selection)).then(clearSelection)}
                    onSnooze={(until) => void bulk.snooze(selectedIds(entries, selection), until).then(clearSelection)}
                    onResolve={(decision) => void bulk.resolve(approvalsOf(selectedItems), decision).then(clearSelection)}
                    onClear={clearSelection}
                  />
                )}
              </>
            )}
          </aside>
          <main className={cn('min-h-0 min-w-0 flex-1 flex-col md:flex', active ? 'flex' : 'hidden')}>
            <Outlet />
          </main>
        </div>
      </article>
    </ViewContext.Provider>
  )
}
