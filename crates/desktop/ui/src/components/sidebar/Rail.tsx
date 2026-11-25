import { Capability } from '@/rpc/capabilities'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import { Briefcase, ChevronDown, ChevronRight, MessagesSquare, MoreHorizontal, Pin, Plus, Zap } from 'lucide-react'
import { type KeyboardEvent, type PointerEvent, type ReactNode, useEffect, useMemo, useRef, useState } from 'react'
import { notify as toast } from '@/lib/notify'
import { openNewChat } from '@/components/chat/NewChatPane'
import { useSessionDrag } from '@/components/chat/Panes'
import { errorText } from '@/components/settings/shared'
import { keys } from '@/components/tasks/queries'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { useSidebar } from '@/components/ui/sidebar'
import { queryKeys, useApi } from '@/lib/rpc'
import { revealSection } from '@/lib/sidebar-reveal'
import { chatLeaves, CHAT, isDraft, PERSIST, STORAGE_KEY, usePanes } from '@/lib/panes'
import { live, notAdvisor, notStudy, rootOf } from '@/lib/session-tree'
import { cn } from '@/lib/utils'
import type { SessionSummary } from '@/rpc/bindings/method.agent.output'
import { type SessionSlice, useSessionField, useSessions } from '@/store/sessions'
import { AgentTree, ago } from './AgentTree'
import { useDeskList } from './Desks'
import { ProjectIcon } from './ProjectStyle'
import { PinnedGlyph, type RowEdit, SessionActivity, SessionEdit, SessionMenu, SessionPending, subagentRunning, useRowTitle, useSessionActions } from './SessionItem'
import { buildRows, newChatLabel, ProjectMenu, type Row, UNGROUPED, useSidebarSessions } from './SessionList'

/** One icon of the collapsed rail: 36 px under a mouse, 44 px under a finger (the rail is 56 px). */
export const RAIL_ITEM = cn(
  'relative flex size-9 shrink-0 items-center justify-center rounded-[10px] text-fg-secondary ring-sidebar-ring outline-hidden pointer-coarse:size-11',
  'hover:bg-sidebar-accent hover:text-sidebar-accent-foreground focus-visible:ring-2',
  'data-[active=true]:bg-sidebar-accent data-[active=true]:text-sidebar-accent-foreground data-[state=open]:bg-sidebar-accent data-[state=open]:text-sidebar-accent-foreground',
  "[&_svg:not([class*='size-'])]:size-[18px]",
)

/**
 * One corner accent, 12 px, centered on the icon's top-right. Parent is the icon box
 * (`relative inline-flex`), not the rail button — the button grows under a finger and the
 * icon stays 18 px. The sidebar ring is a gap, not part of the mark. A count uses
 * {@link ICON_COUNT} (same height, width may grow). The note plus uses {@link ICON_PLUS}:
 * Lucide's plus only spans 14/24 of its box, so a 20 px glyph puts the arm tips on this
 * same 12 px.
 */
export const ICON_MARK = 'pointer-events-none absolute -top-1.5 -right-1.5 size-3 rounded-full ring-2 ring-sidebar'

/** The same mark, on the icon's bottom-right (a plugin dot sharing the corner with a count). */
export const ICON_MARK_BOTTOM = 'pointer-events-none absolute -right-1.5 -bottom-1.5 size-3 rounded-full ring-2 ring-sidebar'

/** {@link ICON_MARK} for a digit: height stays 12 px, width grows past one digit. */
export const ICON_COUNT =
  'pointer-events-none absolute -top-1.5 -right-1.5 ml-0 flex h-3 min-w-3 items-center justify-center rounded-full px-0.5 text-center text-[10px] leading-none font-semibold tabular-nums ring-2 ring-sidebar'

/** The note plus, arm tips spanning the same 12 px as {@link ICON_MARK}. See the comment there. */
export const ICON_PLUS = 'pointer-events-none absolute -top-2.5 -right-2.5 size-5'

/**
 * Puts the rail's scrolling middle back in view and has WebKit rebuild its scroll layer. The rail mounts while the
 * sidebar container is still animating its width down to the rail's; on iPadOS that left the middle (Chat … the
 * section icons) blank between the header and Settings. Undoes any sideways scroll (overflow-x is hidden but still
 * scrolls programmatically), clamps a scroll past the end, and flips `overflow-y` off for a forced layout and back,
 * which drops and re-creates the native scroll view at the settled size.
 */
export function settleRail(nav: HTMLElement): void {
  nav.scrollLeft = 0
  const max = Math.max(0, nav.scrollHeight - nav.clientHeight)
  if (nav.scrollTop > max) nav.scrollTop = max
  const prev = nav.style.overflowY
  nav.style.overflowY = 'hidden'
  void nav.offsetHeight
  nav.style.overflowY = prev
}

/** `settleRail` on the rail's middle once it mounts (next frame) and whenever the sidebar container ends a width transition. */
export function useSettledRail(nav: HTMLElement | null): void {
  useEffect(() => {
    if (!nav) return
    const container = nav.closest<HTMLElement>('[data-slot="sidebar-container"]')
    const frame = requestAnimationFrame(() => settleRail(nav))
    const ended = (e: TransitionEvent) => e.target === container && e.propertyName === 'width' && settleRail(nav)
    container?.addEventListener('transitionend', ended)
    return () => {
      cancelAnimationFrame(frame)
      container?.removeEventListener('transitionend', ended)
    }
  }, [nav])
}

/** Chats a section's popover lists before "Show all N →". */
export const POPOVER_CAP = 8
/** How long a hover-opened popover outlives the pointer, so it can cross from the icon into the popover. */
export const CLOSE_DELAY = 150

export type RailSection = {
  /** The expanded list's header row key (`SessionList`'s reveal target). */
  key: string
  kind: 'active' | 'pinned' | 'project' | 'ungrouped'
  project: string | null
  label: string
  items: { session: SessionSummary; recent: boolean }[]
}

/**
 * The expanded list's sections, in its order (`buildRows` with nothing collapsed or open): Active, Pinned, each project,
 * Ungrouped. Day headers fold into their section; without projects the plain list is one `Chats` section (key `p:`).
 * Archived stays out, as it sits collapsed at the very bottom of the list.
 */
export function railSections(rows: readonly Row[]): RailSection[] {
  const out: RailSection[] = []
  let cur: RailSection | undefined
  const start = (s: Omit<RailSection, 'items'>) => out.push((cur = { ...s, items: [] }))
  for (const r of rows) {
    if ('archived' in r) break
    if ('project' in r) start({ key: r.key, kind: r.project === null ? 'ungrouped' : 'project', project: r.project, label: r.project ?? UNGROUPED })
    else if ('label' in r) {
      if (r.key === 'h:active') start({ key: r.key, kind: 'active', project: null, label: 'Active' })
      else if (r.key === 'h:pinned') start({ key: r.key, kind: 'pinned', project: null, label: 'Pinned' })
      else if (!cur || cur.kind === 'active' || cur.kind === 'pinned') start({ key: 'p:', kind: 'ungrouped', project: null, label: 'Chats' })
    } else if ('session' in r) cur?.items.push({ session: r.session, recent: r.recent === true })
  }
  return out
}

type Via = 'hover' | 'click' | 'key'

/**
 * The one open section popover. A mouse opens it by hovering the icon and keeps it while crossing into it; leaving
 * either closes it `CLOSE_DELAY` later, unless a click (or Enter/Space) pinned it. A finger taps it open and taps
 * outside to close it (Radix). Only a keyboard open moves focus into it.
 */
function useRailPopover() {
  const [open, setOpen] = useState<{ key: string; via: Via } | null>(null)
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  const via = useRef<Via>('hover')
  const escaped = useRef<string | null>(null)
  useEffect(() => () => clearTimeout(timer.current), [])
  const cancel = () => clearTimeout(timer.current)
  const show = (key: string, how: Via) => {
    cancel()
    via.current = how
    setOpen({ key, via: how })
  }
  return {
    open,
    via,
    cancel,
    show,
    close: () => {
      cancel()
      setOpen(null)
    },
    /** A menu, editor or dialog inside opened: the popover stays until it is done with, as if clicked open. */
    pin: () => {
      cancel()
      setOpen((o) => (o?.via === 'hover' ? { ...o, via: 'click' } : o))
    },
    /** The popover Esc closed: the one whose focus goes back to its icon. Any other close (a chat, +, Show all, a click
     * elsewhere) leaves focus where that leads — a new chat's composer — read when Radix restores focus, a tick later. */
    escaped,
    /** Radix dismissing one popover (Esc, outside, focus moved on): only if it is still the open one, not the next. */
    dismiss: (key: string) => setOpen((o) => (o?.key === key ? null : o)),
    enter: (key: string) => (e: PointerEvent) => {
      if (e.pointerType !== 'mouse') return
      if (open?.key === key) cancel()
      else show(key, 'hover')
    },
    leave: (e: PointerEvent) => {
      if (e.pointerType !== 'mouse') return
      cancel()
      // Not while a row of it is being dragged: its source row holds the pointer capture, and unmounting it would end the drag.
      timer.current = setTimeout(() => !usePanes.getState().drag && setOpen((o) => (o?.via === 'hover' ? null : o)), CLOSE_DELAY)
    },
    click: (key: string, how: Via) => {
      if (open?.key === key && open.via !== 'hover') {
        cancel()
        setOpen(null)
      } else show(key, how)
    },
  }
}
type Pop = ReturnType<typeof useRailPopover>

/** ↑/↓ (and Home/End) between a popover's rows, wrapping. Not inside an editor, nor from a menu portaled out of it. */
function arrows(e: KeyboardEvent<HTMLElement>) {
  if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(e.key)) return
  const target = e.target as HTMLElement
  if (e.defaultPrevented || !e.currentTarget.contains(target) || target.tagName === 'INPUT') return
  const items = [...e.currentTarget.querySelectorAll<HTMLElement>('[data-rail-item]')]
  if (!items.length) return
  e.preventDefault()
  const i = items.indexOf(document.activeElement as HTMLElement)
  const n = items.length
  const next = e.key === 'Home' ? 0 : e.key === 'End' ? n - 1 : e.key === 'ArrowDown' ? (i + 1) % n : i <= 0 ? n - 1 : i - 1
  items[next]?.focus()
}

/**
 * A section icon and its popover, to the right of the rail, top-aligned with the icon and kept inside the window.
 * → opens it from the keyboard too. `go` (Active): a click goes there instead of pinning the popover — a mouse at
 * once (hover already showed it), a finger on its second tap, once the first has opened it.
 */
function RailPopover({
  id,
  pop,
  label,
  active,
  icon,
  go,
  children,
}: {
  id: string
  pop: Pop
  label: string
  active?: boolean
  icon: ReactNode
  go?: () => void
  children: ReactNode
}) {
  const pointer = useRef<string>('mouse')
  // A row dragged out of it toward a pane: the popover stays mounted (the row holds the pointer capture; unmounting
  // it would end the drag) but steps aside — unseen and unhit, so `targetAt` finds the panes under it.
  const dragging = usePanes((s) => s.drag !== null)
  return (
    <Popover open={pop.open?.key === id} onOpenChange={(o) => !o && pop.dismiss(id)}>
      <PopoverTrigger asChild>
        <button
          type="button"
          className={RAIL_ITEM}
          aria-label={label}
          data-active={active || undefined}
          data-rail-section={id}
          onPointerDown={(e) => (pointer.current = e.pointerType)}
          onPointerEnter={pop.enter(id)}
          onPointerLeave={pop.leave}
          onKeyDown={(e) => {
            if (e.key !== 'ArrowRight') return
            e.preventDefault()
            pop.show(id, 'key')
          }}
          onClick={(e) => {
            // Ours, not Radix's toggle: a hover-opened popover stays open (pinned) on a click.
            e.preventDefault()
            const how = e.detail === 0 ? 'key' : 'click'
            const finger = how === 'click' && pointer.current !== 'mouse'
            pointer.current = 'mouse'
            if (go && (!finger || pop.open?.key === id)) {
              pop.close()
              go()
            } else pop.click(id, how)
          }}
        >
          {icon}
        </button>
      </PopoverTrigger>
      <PopoverContent
        side="right"
        align="start"
        sideOffset={10}
        alignOffset={-6}
        collisionPadding={8}
        aria-label={label}
        data-dragging={dragging || undefined}
        className={cn('flex max-h-(--radix-popover-content-available-height) w-80 flex-col overflow-y-auto rounded-xl p-1.5', dragging && 'pointer-events-none invisible')}
        onPointerEnter={(e) => e.pointerType === 'mouse' && pop.cancel()}
        onPointerLeave={pop.leave}
        onOpenAutoFocus={(e) => pop.via.current !== 'key' && e.preventDefault()}
        onEscapeKeyDown={(e) => {
          // Esc in a row's rename field ends the edit, not the popover.
          if ((e.target as HTMLElement | null)?.tagName === 'INPUT') e.preventDefault()
          else pop.escaped.current = id
        }}
        onCloseAutoFocus={(e) => {
          if (pop.escaped.current === id) pop.escaped.current = null
          else e.preventDefault()
        }}
        onKeyDown={arrows}
      >
        {children}
      </PopoverContent>
    </Popover>
  )
}

const ICON_BUTTON =
  'flex size-7 shrink-0 items-center justify-center rounded-md text-fg-secondary outline-hidden hover:bg-accent hover:text-accent-foreground focus-visible:bg-accent data-[state=open]:bg-accent pointer-coarse:size-9'

function PopHeader({ icon, label, count, add, menu, extra }: { icon: ReactNode; label: string; count: number; add?: { label: string; run(): void }; menu?: ReactNode; extra?: ReactNode }) {
  return (
    <div className="mb-1 flex items-center gap-2 border-b border-border-subtle px-2 pt-1 pb-1.5">
      {icon}
      <span className="min-w-0 truncate text-sm font-semibold">{label}</span>
      <span className="shrink-0 text-xs text-fg-tertiary tabular-nums">{count}</span>
      <span className="ml-auto flex shrink-0 items-center gap-0.5">
        {extra}
        {add && (
          <button type="button" data-rail-item aria-label={add.label} title={add.label} className={ICON_BUTTON} onClick={add.run}>
            <Plus className="size-4" />
          </button>
        )}
        {menu}
      </span>
    </div>
  )
}

/** The project header's ⋯ menu (`ProjectMenu`), next to the +; the popover stays while it or its dialog is open. */
function PopProjectMenu({ project, pop, onNew }: { project: string; pop: Pop; onNew(): void }) {
  const [open, setOpen] = useState(false)
  return (
    <ProjectMenu
      project={project}
      newLabel={newChatLabel(project)}
      onNew={onNew}
      open={open}
      onOpenChange={(o) => {
        setOpen(o)
        if (o) pop.pin()
      }}
      trigger={
        <button type="button" aria-label="Project options" title="Project options" className={ICON_BUTTON} onPointerDown={() => pop.pin()}>
          <MoreHorizontal className="size-4" />
        </button>
      }
    />
  )
}

const POP_ROW =
  'flex h-8 w-full items-center gap-2 rounded-md px-2 text-left text-[13px] text-fg-secondary outline-hidden hover:bg-accent hover:text-accent-foreground focus-visible:bg-accent focus-visible:text-accent-foreground data-[active=true]:font-medium data-[active=true]:text-foreground pointer-coarse:h-10'

/** The running subagents of `id`, any depth: live in its slice's tree (`children`) or, before this client saw them,
 * running in `list_sessions` (`subs`). Joined inside the selector so a streamed delta does not re-render the row. */
function useRunningSubagents(id: string, subs: readonly SessionSummary[] | undefined): { id: string; name: string }[] {
  const key = useSessions((st) => runningKey(id, subs, st.sessions))
  return useMemo(() => (key ? key.split('\n').map((l) => { const [sid = '', name = ''] = l.split('\t'); return { id: sid, name } }) : []), [key])
}

function runningKey(id: string, subs: readonly SessionSummary[] | undefined, slices: Record<string, SessionSlice>): string {
  const out = new Map<string, string>()
  for (const r of Object.values(slices[id]?.children ?? {})) if (live(r.state.state)) out.set(r.id, r.name)
  for (const d of subs ?? []) if (!out.has(d.id) && subagentRunning(d, slices)) out.set(d.id, d.name ?? d.title)
  return [...out].map(([sid, name]) => `${sid}\t${name}`).join('\n')
}

/** A link inside the row's popover area closes the popover (a subagent of the tree, say). */
const closeOnLink = (onPick: () => void) => (e: { target: EventTarget }) => {
  if ((e.target as HTMLElement).closest('a')) onPick()
}

/**
 * One chat of a popover: the expanded row's glyphs and title, then how long since it last changed, and its ⋯ menu
 * (`SessionMenu`: shown on hover or focus, always under a finger). A chat open in a pane shows its live subagents
 * below it (`AgentTree`); any other with subagents running shows `⑂ N` and a ▸ that lists them inline.
 */
function PopChat({
  s,
  recent,
  inPane,
  subagents,
  active,
  here,
  pop,
}: {
  s: SessionSummary
  recent: boolean
  inPane: boolean
  subagents: readonly SessionSummary[] | undefined
  active: boolean
  here: string | undefined
  pop: Pop
}) {
  const title = useRowTitle(s.id, s.title, s.created_at)
  // The expanded row's drag source: the same press-drag into a pane (`useSessionDrag`); a drag swallows the click.
  const drag = useSessionDrag(s.id, title)
  const [editing, setEditing] = useState<RowEdit | null>(null)
  const [menu, setMenu] = useState(false)
  const [confirming, setConfirming] = useState(false)
  const [expanded, setExpanded] = useState(false)
  const actions = useSessionActions(s.id, { shown: title, project: s.project, pinned: s.pinned, archived: s.archived, active })
  const running = useRunningSubagents(s.id, subagents)
  const hold = (o: boolean) => o && pop.pin()
  if (editing) {
    return (
      <SessionEdit
        editing={editing}
        setEditing={(e) => {
          if (e) pop.pin()
          setEditing(e)
        }}
        actions={actions}
        className="h-8 text-[13px]"
      />
    )
  }
  return (
    <>
      <div className={cn('group/pop-row relative flex items-center rounded-md', recent && 'opacity-70')}>
        <Link to="/c/$session" params={{ session: s.id }} {...drag} data-rail-item data-active={active || undefined} title={title} className={cn(POP_ROW, 'min-w-0 flex-1')} onClick={pop.close}>
          {s.pinned && <PinnedGlyph />}
          <SessionActivity id={s.id} here={s.here} running={s.running} />
          <SessionPending id={s.id} />
          <span className="min-w-0 flex-1 truncate">{title}</span>
          <span className="shrink-0 text-[11px] text-fg-tertiary tabular-nums">{ago(s.updated_at)}</span>
        </Link>
        {!inPane && running.length > 0 && (
          <button
            type="button"
            aria-expanded={expanded}
            aria-label={`${running.length} subagent${running.length === 1 ? '' : 's'} running`}
            title={`${running.length} subagent${running.length === 1 ? '' : 's'} running`}
            className="flex h-6 shrink-0 items-center gap-0.5 rounded-md px-1 text-[11px] font-medium text-success tabular-nums hover:bg-accent"
            onClick={() => {
              pop.pin()
              setExpanded((v) => !v)
            }}
          >
            ⑂ {running.length}
            {expanded ? <ChevronDown className="size-3" /> : <ChevronRight className="size-3" />}
          </button>
        )}
        <SessionMenu
          id={s.id}
          shown={title}
          pinned={s.pinned}
          archived={s.archived}
          project={s.project}
          here={s.here}
          actions={actions}
          open={menu}
          onOpenChange={(o) => {
            hold(o)
            setMenu(o)
          }}
          confirming={confirming}
          onConfirmingChange={(o) => {
            hold(o)
            setConfirming(o)
          }}
          onEdit={(e) => {
            pop.pin()
            setEditing(e)
          }}
          trigger={
            <button
              type="button"
              aria-label="Chat options"
              title="Chat options"
              className={cn(
                ICON_BUTTON,
                'size-6 pointer-fine:opacity-0 pointer-fine:group-hover/pop-row:opacity-100 pointer-fine:group-focus-within/pop-row:opacity-100 pointer-fine:data-[state=open]:opacity-100',
              )}
              onPointerDown={() => pop.pin()}
            >
              <MoreHorizontal className="size-4" />
            </button>
          }
        />
      </div>
      {inPane ? (
        <div onClickCapture={closeOnLink(pop.close)}>
          <AgentTree session={s.id} active={here} liveOnly />
        </div>
      ) : (
        expanded &&
        running.length > 0 && (
          <ul aria-label="Running subagents">
            {running.map((r) => (
              <li key={r.id}>
                <PopSubagent {...r} onPick={pop.close} />
              </li>
            ))}
          </ul>
        )
      )}
    </>
  )
}

/** A running subagent listed under its chat: opens it, or drags into a pane like the expanded tree's rows. */
function PopSubagent({ id, name, onPick }: { id: string; name: string; onPick(): void }) {
  const drag = useSessionDrag(id, name)
  return (
    <Link to="/c/$session" params={{ session: id }} {...drag} data-rail-item className={cn(POP_ROW, 'h-7 pl-6 text-xs')} onClick={onPick}>
      <span className="size-1.5 shrink-0 animate-pulse rounded-full bg-success" aria-hidden />
      <span className="min-w-0 flex-1 truncate">{name}</span>
    </Link>
  )
}

function ShowAll({ count, onClick }: { count: number; onClick(): void }) {
  return (
    <button type="button" data-rail-item className={cn(POP_ROW, 'h-7 text-xs text-fg-tertiary')} onClick={onClick}>
      Show all {count} →
    </button>
  )
}

const sectionIcon = (s: RailSection, size: string) =>
  s.kind === 'active' ? <Zap className={size} /> : s.kind === 'pinned' ? <Pin className={size} /> : s.kind === 'project' ? <ProjectIcon project={s.project} className={size} /> : <MessagesSquare className={size} />

/** The green dot on a section icon: anything of Active running (`running`), or any subagent running under one of its chats. */
function SectionDot({ section, subs, running }: { section: RailSection; subs: ReadonlyMap<string, readonly SessionSummary[]>; running: boolean }) {
  const sub = useSessions((st) => section.items.some(({ session }) => runningKey(session.id, subs.get(session.id), st.sessions) !== ''))
  if (!running && !sub) return null
  return <span className={cn(ICON_MARK, 'bg-success')} aria-label="running" />
}

/** The newest of `ids` by `updated_at` among `sessions`; `undefined` when none is listed. */
const newest = (ids: readonly string[], sessions: readonly SessionSummary[]): string | undefined =>
  sessions.filter((s) => ids.includes(s.id)).sort((a, b) => b.updated_at.localeCompare(a.updated_at))[0]?.id

/** The chat leaves of the layout `Panes` persisted (`theuth.panes`), for when no workspace has mounted since the load. */
export function storedChats(): string[] {
  if (!PERSIST) return []
  try {
    const doc = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? 'null') as { views?: Record<string, { type?: string }> } | null
    return Object.entries(doc?.views ?? {}).flatMap(([id, v]) => (v?.type === CHAT ? [id] : []))
  } catch {
    return []
  }
}

/**
 * Where the Active icon goes: the chat workspace as it was left. The layout's focused chat, else its most recent chat
 * leaf (this session's layout, else the persisted one); then the most recently active chat; `null` (→ `/new`) when
 * there is no chat at all. Drafts and chats no longer listed never count, so landing adds no pane.
 */
export function workspaceTarget({
  leaves,
  focused,
  stored,
  sessions,
}: {
  leaves: readonly string[]
  focused: string | null
  stored: readonly string[]
  sessions: readonly SessionSummary[]
}): string | null {
  const listed = new Set(sessions.map((s) => s.id))
  const ok = (id: string): boolean => !isDraft(id) && listed.has(id)
  const laid = chatLeaves(leaves).filter(ok)
  if (focused !== null && laid.includes(focused)) return focused
  const leaf = newest(laid, sessions) ?? newest(stored.filter(ok), sessions)
  if (leaf) return leaf
  const roots = sessions.filter((s) => s.parent === null && s.desk == null && !s.archived && notAdvisor(s) && notStudy(s))
  return newest(roots.map((s) => s.id), roots) ?? null
}

/**
 * The rail's lower half: one icon per section the expanded list shows (Active, Pinned, each project in its own glyph
 * and colour, Ungrouped; a green dot while something under it runs), then Desks. Each opens a popover of its chats;
 * Active's click opens the chat workspace (`workspaceTarget`).
 */
export function RailSections({ active }: { active: string | undefined }) {
  const navigate = useNavigate()
  const { setOpen } = useSidebar()
  const { sessions, subs, activeSet, recent } = useSidebarSessions()
  const sections = useMemo(() => railSections(buildRows(sessions, [], undefined, false, activeSet, recent)), [sessions, activeSet, recent])
  const here = useMemo(() => rootOf(sessions, active), [sessions, active])
  const leaves = usePanes((s) => s.leaves)
  const inPane = useMemo(() => new Set([...chatLeaves(leaves), ...(active ? [active] : [])]), [leaves, active])
  const pop = useRailPopover()
  // A drag out of the popover ended (dropped on a pane or cancelled): the popover it stepped aside from closes.
  const dragging = usePanes((s) => s.drag !== null)
  const dragged = useRef(false)
  useEffect(() => {
    if (dragging) dragged.current = pop.open !== null
    else if (dragged.current) {
      dragged.current = false
      pop.close()
    }
  }, [dragging]) // eslint-disable-line react-hooks/exhaustive-deps
  const showAll = (key: string) => {
    pop.close()
    revealSection(key)
    setOpen(true)
  }
  const workspace = () => {
    const { leaves, focused } = usePanes.getState()
    const target = workspaceTarget({ leaves, focused, stored: storedChats(), sessions })
    void (target ? navigate({ to: '/c/$session', params: { session: target } }) : navigate({ to: '/new' }))
  }
  return (
    <>
      {sections.map((s) => {
        const onNew = () => {
          pop.close()
          openNewChat(navigate, s.project)
        }
        const add = s.kind === 'project' || s.kind === 'ungrouped' ? { label: newChatLabel(s.project), run: onNew } : undefined
        return (
          <RailPopover
            key={s.key}
            id={s.key}
            pop={pop}
            label={`${s.label}, ${s.items.length} chat${s.items.length === 1 ? '' : 's'}`}
            active={here !== undefined && s.items.some((i) => i.session.id === here)}
            go={s.kind === 'active' ? workspace : undefined}
            icon={
              <span className="relative inline-flex">
                {sectionIcon(s, 'size-[18px]')}
                <SectionDot section={s} subs={subs} running={s.kind === 'active' && activeSet.size > 0} />
              </span>
            }
          >
            <PopHeader
              icon={sectionIcon(s, 'size-4')}
              label={s.label}
              count={s.items.length}
              add={add}
              menu={s.kind === 'project' && s.project !== null ? <PopProjectMenu project={s.project} pop={pop} onNew={onNew} /> : undefined}
              extra={
                s.kind === 'active' ? (
                  <button
                    type="button"
                    data-rail-item
                    className="h-7 rounded-md px-2 text-xs text-fg-secondary outline-hidden hover:bg-accent hover:text-accent-foreground focus-visible:bg-accent pointer-coarse:h-9"
                    onClick={() => {
                      pop.close()
                      workspace()
                    }}
                  >
                    Open workspace
                  </button>
                ) : undefined
              }
            />
            <ul>
              {s.items.slice(0, POPOVER_CAP).map(({ session, recent }) => (
                <li key={session.id}>
                  <PopChat s={session} recent={recent} inPane={inPane.has(session.id)} subagents={subs.get(session.id)} active={session.id === here} here={active} pop={pop} />
                </li>
              ))}
            </ul>
            {s.items.length > POPOVER_CAP && <ShowAll count={s.items.length} onClick={() => showAll(s.key)} />}
          </RailPopover>
        )
      })}
      <Capability required={{ methods: ['desk_open', 'list_sessions'] }} hide>
        <RailDesks pop={pop} active={active} onShowAll={() => showAll('desks')} />
      </Capability>
    </>
  )
}

/** The Desks group as one rail icon; its popover lists the desks with their working dot. */
function RailDesks({ pop, active, onShowAll }: { pop: Pop; active: string | undefined; onShowAll(): void }) {
  const desks = useDeskList()
  if (desks.length === 0) return null
  return (
    <RailPopover
      id="desks"
      pop={pop}
      label={`Desks, ${desks.length}`}
      active={desks.some((d) => d.id !== null && d.id === active)}
      icon={<Briefcase />}
    >
      <PopHeader icon={<Briefcase className="size-4" />} label="Desks" count={desks.length} />
      <ul>
        {desks.slice(0, POPOVER_CAP).map((d) => (
          <li key={d.name}>
            <PopDesk {...d} active={d.id !== null && d.id === active} onPick={pop.close} />
          </li>
        ))}
      </ul>
      {desks.length > POPOVER_CAP && <ShowAll count={desks.length} onClick={onShowAll} />}
    </RailPopover>
  )
}

/** A desk row: opens its session, or asks the roster to open the desk first (as its tile does). */
function PopDesk({ id, name, working, active, onPick }: { id: string | null; name: string; working: boolean; active: boolean; onPick(): void }) {
  const api = useApi()
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const running = useSessionField(id ?? '', 'running')
  const open = useMutation({
    mutationFn: () => api.desk_open(name),
    onSuccess: (session) => {
      void queryClient.invalidateQueries({ queryKey: keys.agents })
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions })
      void navigate({ to: '/c/$session', params: { session } })
    },
    onError: (e) => toast.error(errorText(e)),
  })
  const busy = working || running === true
  const body = (
    <>
      <span className={cn('size-1.5 shrink-0 rounded-full', busy ? 'bg-success' : 'bg-fg-tertiary/40')} aria-hidden />
      <span className="min-w-0 flex-1 truncate">{name}</span>
      <span className="shrink-0 text-[11px] text-fg-tertiary">{open.isPending ? 'opening' : busy ? 'working' : 'idle'}</span>
    </>
  )
  return id === null ? (
    <button
      type="button"
      data-rail-item
      className={POP_ROW}
      disabled={open.isPending}
      onClick={() => {
        onPick()
        open.mutate()
      }}
    >
      {body}
    </button>
  ) : (
    <Link to="/c/$session" params={{ session: id }} data-rail-item data-active={active || undefined} className={POP_ROW} onClick={onPick}>
      {body}
    </Link>
  )
}
