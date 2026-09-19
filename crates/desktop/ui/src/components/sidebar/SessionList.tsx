import { useQuery } from '@tanstack/react-query'
import { ChevronDown, ChevronRight } from 'lucide-react'
import { useNavigate } from '@tanstack/react-router'
import { useVirtualizer } from '@tanstack/react-virtual'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { toast } from 'sonner'
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
import { leaves, usePanes } from '@/lib/panes'
import { queryKeys, useApi } from '@/lib/rpc'
import { nextSelection, type SelectMode } from '@/lib/selection'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import { AgentTree } from './AgentTree'
import { SessionItem } from './SessionItem'

export type Row =
  | { key: string; label: string }
  | { key: string; project: string | null; collapsed: boolean }
  | { key: string; session: SessionSummary }
  | { key: string; tree: string }

/** Sidebar name of the `null` project bucket (51 §3). */
export const UNGROUPED = 'Ungrouped'

const DAY = 86_400_000

/** Today / Yesterday / Previous 7 days / Older, by local calendar day of `updated_at`. */
function group(iso: string, now: number): string {
  const start = new Date(now).setHours(0, 0, 0, 0)
  const t = Date.parse(iso)
  if (t >= start) return 'Today'
  if (t >= start - DAY) return 'Yesterday'
  if (t >= start - 7 * DAY) return 'Previous 7 days'
  return 'Older'
}

/** The top-level ancestor of `id` (itself when top-level or unknown). */
function rootOf(sessions: SessionSummary[], id: string | undefined): string | undefined {
  let cur = id
  for (;;) {
    const parent = sessions.find((s) => s.id === cur)?.parent
    if (!parent) return cur
    cur = parent
  }
}

/**
 * Pinned top-level sessions first, then the rest newest first with day-group headers; the root of every open
 * chat (the URL's and each split pane's) is followed by its agent tree. With any project set (51 §3), roots are
 * bucketed by project in first-seen order of that ordering, `Ungrouped` last, under collapsible project headers
 * (`collapsed` holds project labels, `''` = ungrouped).
 */
export function buildRows(sessions: SessionSummary[], open: readonly string[], collapsed: ReadonlySet<string> = new Set()): Row[] {
  const now = Date.now()
  const roots = new Set(open.map((id) => rootOf(sessions, id)))
  const rows: Row[] = []
  const top = sessions
    .filter((s) => s.parent === null)
    .sort((a, b) => Number(b.pinned) - Number(a.pinned) || b.updated_at.localeCompare(a.updated_at))
  const buckets = new Map<string | null, SessionSummary[]>()
  for (const s of top) {
    const list = buckets.get(s.project)
    if (list) list.push(s)
    else buckets.set(s.project, [s])
  }
  const ungrouped = buckets.get(null)
  buckets.delete(null)
  if (ungrouped) buckets.set(null, ungrouped)
  const grouped = buckets.size > 1 || !ungrouped
  for (const [project, list] of buckets) {
    if (grouped) {
      const isCollapsed = collapsed.has(project ?? '')
      rows.push({ key: `p:${project ?? ''}`, project, collapsed: isCollapsed })
      if (isCollapsed) continue
    }
    let label = ''
    for (const s of list) {
      const g = s.pinned ? 'Pinned' : group(s.updated_at, now)
      if (g !== label) rows.push({ key: `h:${project ?? ''}:${g}`, label: (label = g) })
      rows.push({ key: s.id, session: s })
      if (roots.has(s.id)) rows.push({ key: `t:${s.id}`, tree: s.id })
    }
  }
  return rows
}

/**
 * The chat list (`list_sessions`), virtualized on the sidebar's scroll container.
 * ⌘-click toggles a chat into a selection, ⇧-click extends it from the last one
 * (Finder rules); a bar under the list deletes them together. Esc clears.
 */
export function SessionList({ scrollEl, active }: { scrollEl: HTMLDivElement | null; active: string | undefined }) {
  const api = useApi()
  const navigate = useNavigate()
  const { data: sessions = [] } = useQuery({ queryKey: queryKeys.sessions, queryFn: () => api.list_sessions() })
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(() => new Set())
  const root = usePanes((s) => s.root)
  const open = useMemo(() => (root ? leaves(root) : active ? [active] : []), [root, active])
  const rows = useMemo(() => buildRows(sessions, open, collapsed), [sessions, open, collapsed])
  const [selected, setSelected] = useState<ReadonlySet<string>>(() => new Set())
  const [confirming, setConfirming] = useState(false)
  // Visible top-level order for ⇧-click ranges, and the range's start; refs so
  // `select` stays stable and the memoized items do not re-render per list change.
  const order = useRef<string[]>([])
  order.current = rows.flatMap((r) => ('session' in r ? [r.session.id] : []))
  const anchor = useRef<string | null>(null)

  const select = useCallback((id: string, mode: SelectMode) => {
    if (mode === 'toggle') anchor.current = id
    setSelected((prev) => nextSelection(prev, order.current, anchor.current, id, mode))
  }, [])

  useEffect(() => {
    if (!selected.size) return
    // Radix prevents default when Esc closes the confirm dialog: that press must not also drop the selection.
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !e.defaultPrevented) select('', 'clear')
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [selected.size > 0, select])

  const remove = async () => {
    const ids = [...selected]
    const results = await Promise.allSettled(ids.map((id) => api.delete_session(id)))
    const failed = ids.filter((_, i) => results[i]?.status === 'rejected')
    setSelected(new Set(failed))
    if (failed.length) {
      const first = results.find((r) => r.status === 'rejected')?.reason
      toast.error(`${failed.length} of ${ids.length} not deleted: ${first instanceof Error ? first.message : String(first)}`)
    }
    const root = rootOf(sessions, active)
    if (root && ids.includes(root) && !failed.includes(root)) void navigate({ to: '/' })
  }

  const v = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollEl,
    estimateSize: (i) => ('session' in (rows[i] ?? {}) || 'tree' in (rows[i] ?? {}) ? 40 : 36),
    overscan: 8,
    getItemKey: (i) => rows[i]?.key ?? i,
  })
  return (
    <>
      <ul className="relative block px-1.5" style={{ height: v.getTotalSize() }}>
        {v.getVirtualItems().map((item) => {
          const row = rows[item.index]
          if (!row) return null
          return (
            <li
              key={item.key}
              data-index={item.index}
              ref={v.measureElement}
              className="absolute top-0 left-0 w-full"
              style={{ transform: `translateY(${item.start}px)` }}
            >
              {'project' in row ? (
                <button
                  type="button"
                  className="flex w-full items-center gap-1 px-2.5 pt-4 pb-1.5 text-xs font-semibold text-fg-secondary"
                  aria-expanded={!row.collapsed}
                  onClick={() =>
                    setCollapsed((prev) => {
                      const next = new Set(prev)
                      const k = row.project ?? ''
                      if (!next.delete(k)) next.add(k)
                      return next
                    })
                  }
                >
                  {row.collapsed ? <ChevronRight className="size-3" /> : <ChevronDown className="size-3" />}
                  <span className="truncate">{row.project ?? UNGROUPED}</span>
                </button>
              ) : 'label' in row ? (
                <div className="px-2.5 pt-4 pb-1.5 text-xs font-medium text-fg-tertiary">{row.label}</div>
              ) : 'session' in row ? (
                <SessionItem
                  id={row.session.id}
                  title={row.session.title}
                  pinned={row.session.pinned}
                  project={row.session.project}
                  active={row.session.id === active}
                  selected={selected.has(row.session.id)}
                  selecting={selected.size > 0}
                  select={select}
                />
              ) : (
                <AgentTree session={row.tree} active={active} />
              )}
            </li>
          )
        })}
      </ul>
      {selected.size > 0 && (
        <div className="sticky bottom-0 mt-2 flex items-center gap-1 border-t border-border-subtle bg-sidebar px-3 py-2 text-sm">
          <span className="flex-1 truncate">{selected.size} selected</span>
          <Button variant="ghost" size="sm" onClick={() => select('', 'clear')}>
            Cancel
          </Button>
          <Button variant="destructive" size="sm" onClick={() => setConfirming(true)}>
            Delete
          </Button>
        </div>
      )}
      <AlertDialog open={confirming} onOpenChange={setConfirming}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete {selected.size === 1 ? 'this chat' : `${selected.size} chats`}?</AlertDialogTitle>
            <AlertDialogDescription>Any subagent chats under them go too. This cannot be undone.</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction onClick={() => void remove()}>Delete</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  )
}
