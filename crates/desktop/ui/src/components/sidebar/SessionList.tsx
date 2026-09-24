import { useQuery, useQueryClient } from '@tanstack/react-query'
import { ChevronDown, ChevronRight, MoreHorizontal, X } from 'lucide-react'
import { useNavigate } from '@tanstack/react-router'
import { useVirtualizer } from '@tanstack/react-virtual'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
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
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { SidebarMenuAction } from '@/components/ui/sidebar'
import { openNewChat } from '@/components/chat/NewChatPane'
import { leaves, usePanes } from '@/lib/panes'
import { queryKeys, useApi } from '@/lib/rpc'
import { nextSelection, type SelectMode } from '@/lib/selection'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import { AgentTree } from './AgentTree'
import { ProjectIcon, ProjectStyleDialog } from './ProjectStyle'
import { SessionItem } from './SessionItem'

export type Row =
  | { key: string; label: string }
  | { key: string; project: string | null; collapsed: boolean }
  | { key: string; archived: number; collapsed: boolean }
  | { key: string; session: SessionSummary }
  | { key: string; tree: string }

/** Sidebar name of the `null` project bucket (51 §3). */
export const UNGROUPED = 'Ungrouped'

/** Menu label of the project header's new-chat action; `null` is the ungrouped bucket. */
export const newChatLabel = (project: string | null) => (project === null ? 'New chat (no project)' : `New chat in “${project}”`)

/**
 * A collapsible project header (51 §3): the row toggles collapse; right-click or the hover `…` (always shown on
 * phones, like `SessionItem`'s) opens a menu whose "New chat in <project>" opens a new chat with the picker preset
 * (`openNewChat`: the focused pane when split, else `/?project=`) and whose "Icon & colour…" opens the style dialog
 * (not for Ungrouped). The project's glyph sits left of the name.
 */
function ProjectHeader({ project, collapsed, toggle }: { project: string | null; collapsed: boolean; toggle(): void }) {
  const navigate = useNavigate()
  const [open, setOpen] = useState(false)
  const [styling, setStyling] = useState(false)
  return (
    <div className="group/menu-item relative">
      <button
        type="button"
        className="flex w-full items-center gap-1 px-2.5 pt-4 pb-1.5 text-xs font-semibold text-fg-secondary"
        aria-expanded={!collapsed}
        onClick={toggle}
        onContextMenu={(e) => {
          e.preventDefault()
          setOpen(true)
        }}
      >
        {collapsed ? <ChevronRight className="size-3" /> : <ChevronDown className="size-3" />}
        <ProjectIcon project={project} className="size-4" />
        <span className="truncate">{project ?? UNGROUPED}</span>
      </button>
      <DropdownMenu open={open} onOpenChange={setOpen}>
        <DropdownMenuTrigger asChild>
          <SidebarMenuAction showOnHover className="top-3.5 right-2" aria-label="Project options">
            <MoreHorizontal />
          </SidebarMenuAction>
        </DropdownMenuTrigger>
        <DropdownMenuContent side="right" align="start" className="w-48">
          <DropdownMenuItem onSelect={() => openNewChat(navigate, project)}>
            {newChatLabel(project)}
          </DropdownMenuItem>
          {project !== null && <DropdownMenuItem onSelect={() => setStyling(true)}>Icon &amp; colour…</DropdownMenuItem>}
          <DropdownMenuItem onSelect={toggle}>{collapsed ? 'Expand' : 'Collapse'}</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      {project !== null && <ProjectStyleDialog project={project} open={styling} onOpenChange={setStyling} />}
    </div>
  )
}

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

/** Every subagent under its top-level ancestor (all depths), for the collapsed row's subagent glyph. */
export function subtrees(sessions: SessionSummary[]): Map<string, SessionSummary[]> {
  const out = new Map<string, SessionSummary[]>()
  for (const s of sessions) {
    if (s.parent === null) continue
    const root = rootOf(sessions, s.id)
    if (root === undefined) continue
    const list = out.get(root)
    if (list) list.push(s)
    else out.set(root, [s])
  }
  return out
}

/**
 * Pinned top-level sessions first under one `Pinned` header, then the rest newest first with day-group headers;
 * the root of every open chat (the URL's and each split pane's) is followed by its agent tree. With any project
 * set (51 §3), unpinned roots are bucketed by project in first-seen order of that ordering, `Ungrouped` last,
 * under collapsible project headers (`collapsed` holds project labels, `''` = ungrouped). Archived roots (51 §3)
 * leave all of that for one `Archived (N)` header at the very bottom, collapsed unless `archivedOpen`; their
 * children only ever render under them (as the tree of an open chat), never as orphans above.
 */
export function buildRows(
  sessions: SessionSummary[],
  open: readonly string[],
  collapsed: ReadonlySet<string> = new Set(),
  archivedOpen = false,
): Row[] {
  const now = Date.now()
  const roots = new Set(open.map((id) => rootOf(sessions, id)))
  const rows: Row[] = []
  const push = (s: SessionSummary) => {
    rows.push({ key: s.id, session: s })
    if (roots.has(s.id)) rows.push({ key: `t:${s.id}`, tree: s.id })
  }
  const all = sessions.filter((s) => s.parent === null).sort((a, b) => b.updated_at.localeCompare(a.updated_at))
  const top = all.filter((s) => !s.archived)
  const pinned = top.filter((s) => s.pinned)
  if (pinned.length) {
    rows.push({ key: 'h:pinned', label: 'Pinned' })
    pinned.forEach(push)
  }
  const buckets = new Map<string | null, SessionSummary[]>()
  for (const s of top) {
    if (s.pinned) continue
    const list = buckets.get(s.project)
    if (list) list.push(s)
    else buckets.set(s.project, [s])
  }
  const ungrouped = buckets.get(null)
  buckets.delete(null)
  if (ungrouped) buckets.set(null, ungrouped)
  const grouped = buckets.size > 1 || (!ungrouped && buckets.size > 0)
  for (const [project, list] of buckets) {
    if (grouped) {
      const isCollapsed = collapsed.has(project ?? '')
      rows.push({ key: `p:${project ?? ''}`, project, collapsed: isCollapsed })
      if (isCollapsed) continue
    }
    let label = ''
    for (const s of list) {
      const g = group(s.updated_at, now)
      if (g !== label) rows.push({ key: `h:${project ?? ''}:${g}`, label: (label = g) })
      push(s)
    }
  }
  const archived = all.filter((s) => s.archived)
  if (archived.length) {
    rows.push({ key: 'p:archived', archived: archived.length, collapsed: !archivedOpen })
    if (archivedOpen) archived.forEach(push)
  }
  return rows
}

/**
 * The chat list (`list_sessions`), virtualized on the sidebar's scroll container.
 * ⌘-click toggles a chat into a selection, ⇧-click extends it from the last one
 * (Finder rules); a bar deletes or archives them together. Esc clears. The bar is
 * portaled into `barEl`, the sidebar's slot below the scroll container, so it stays
 * visible while the list scrolls. (It used to be a `sticky bottom-0` child of the
 * scroller, which is a flex column: the `<ul>` is a shrinkable flex item whose only
 * content is absolutely positioned, so past the viewport its virtualizer height
 * collapsed to the leftover space, the bar's static position landed right under it
 * and sticky — a one-sided constraint — let it scroll away with the list.)
 */
export function SessionList({
  scrollEl,
  barEl,
  active,
}: {
  scrollEl: HTMLDivElement | null
  barEl: HTMLDivElement | null
  active: string | undefined
}) {
  const api = useApi()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const { data: sessions = [] } = useQuery({ queryKey: queryKeys.sessions, queryFn: () => api.list_sessions() })
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(() => new Set())
  const [archivedOpen, setArchivedOpen] = useState(false)
  const root = usePanes((s) => s.root)
  const open = useMemo(() => (root ? leaves(root) : active ? [active] : []), [root, active])
  const rows = useMemo(() => buildRows(sessions, open, collapsed, archivedOpen), [sessions, open, collapsed, archivedOpen])
  const subs = useMemo(() => subtrees(sessions), [sessions])
  const [selected, setSelected] = useState<ReadonlySet<string>>(() => new Set())
  const [confirming, setConfirming] = useState(false)
  // Every selected row is archived: the bar offers Unarchive instead of Archive.
  const allArchived = selected.size > 0 && [...selected].every((id) => sessions.find((s) => s.id === id)?.archived)
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

  const archive = async () => {
    const ids = [...selected]
    const results = await Promise.allSettled(ids.map((id) => api.archive_session(id, !allArchived)))
    const failed = ids.filter((_, i) => results[i]?.status === 'rejected')
    setSelected(new Set(failed))
    if (failed.length) {
      const first = results.find((r) => r.status === 'rejected')?.reason
      toast.error(`${failed.length} of ${ids.length} not ${allArchived ? 'unarchived' : 'archived'}: ${first instanceof Error ? first.message : String(first)}`)
    }
    void qc.invalidateQueries({ queryKey: queryKeys.sessions })
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
      {/* `shrink-0`: a flex item of the scroller; without it the column layout squeezes the measured height away. */}
      <ul className="relative block shrink-0 px-1.5" style={{ height: v.getTotalSize() }}>
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
                <ProjectHeader
                  project={row.project}
                  collapsed={row.collapsed}
                  toggle={() =>
                    setCollapsed((prev) => {
                      const next = new Set(prev)
                      const k = row.project ?? ''
                      if (!next.delete(k)) next.add(k)
                      return next
                    })
                  }
                />
              ) : 'archived' in row ? (
                <button
                  type="button"
                  className="flex w-full items-center gap-1 px-2.5 pt-4 pb-1.5 text-xs font-semibold text-fg-tertiary"
                  aria-expanded={!row.collapsed}
                  onClick={() => setArchivedOpen((prev) => !prev)}
                >
                  {row.collapsed ? <ChevronRight className="size-3" /> : <ChevronDown className="size-3" />}
                  <span className="truncate">Archived ({row.archived})</span>
                </button>
              ) : 'label' in row ? (
                <div className="px-2.5 pt-4 pb-1.5 text-xs font-medium text-fg-tertiary">{row.label}</div>
              ) : 'session' in row ? (
                <SessionItem
                  id={row.session.id}
                  title={row.session.title}
                  tags={row.session.tags}
                  pinned={row.session.pinned}
                  archived={row.session.archived}
                  project={row.session.project}
                  subagents={subs.get(row.session.id)}
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
      {selected.size > 0 &&
        barEl &&
        createPortal(
          <div className="flex items-center gap-0.5 border-t border-border-subtle bg-sidebar px-2 py-2 text-sm">
            <span className="flex-1 truncate pl-1">{selected.size} selected</span>
            <Button variant="ghost" size="icon-sm" aria-label="Clear selection" title="Clear selection" onClick={() => select('', 'clear')}>
              <X />
            </Button>
            <Button variant="ghost" size="sm" onClick={() => void archive()}>
              {allArchived ? 'Unarchive' : 'Archive'}
            </Button>
            <Button variant="destructive" size="sm" onClick={() => setConfirming(true)}>
              Delete
            </Button>
          </div>,
          barEl,
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
