import { Capability } from '@/rpc/capabilities'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import { Briefcase, MessagesSquare, Pin, Plus, Zap } from 'lucide-react'
import { type KeyboardEvent, type PointerEvent, type ReactNode, useEffect, useMemo, useRef, useState } from 'react'
import { toast } from 'sonner'
import { openNewChat } from '@/components/chat/NewChatPane'
import { errorText } from '@/components/settings/shared'
import { keys } from '@/components/tasks/queries'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { useSidebar } from '@/components/ui/sidebar'
import { queryKeys, useApi } from '@/lib/rpc'
import { revealSection } from '@/lib/sidebar-reveal'
import { rootOf } from '@/lib/session-tree'
import { cn } from '@/lib/utils'
import type { SessionSummary } from '@/rpc/bindings/method.agent.output'
import { useSessionField } from '@/store/sessions'
import { ago } from './AgentTree'
import { useDeskList } from './Desks'
import { ProjectIcon } from './ProjectStyle'
import { SessionActivity, SessionPending, SessionSubagents, useRowTitle } from './SessionItem'
import { buildRows, newChatLabel, type Row, UNGROUPED, useSidebarSessions } from './SessionList'

/** One icon of the collapsed rail: 36 px under a mouse, 44 px under a finger (the rail is 56 px). */
export const RAIL_ITEM = cn(
  'relative flex size-9 shrink-0 items-center justify-center rounded-[10px] text-fg-secondary ring-sidebar-ring outline-hidden pointer-coarse:size-11',
  'hover:bg-sidebar-accent hover:text-sidebar-accent-foreground focus-visible:ring-2',
  'data-[active=true]:bg-sidebar-accent data-[active=true]:text-sidebar-accent-foreground data-[state=open]:bg-sidebar-accent data-[state=open]:text-sidebar-accent-foreground',
  "[&_svg:not([class*='size-'])]:size-[18px]",
)

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
    close: () => {
      cancel()
      setOpen(null)
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
      timer.current = setTimeout(() => setOpen((o) => (o?.via === 'hover' ? null : o)), CLOSE_DELAY)
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

/** ↑/↓ (and Home/End) between a popover's rows, wrapping. */
function arrows(e: KeyboardEvent<HTMLElement>) {
  if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(e.key)) return
  const items = [...e.currentTarget.querySelectorAll<HTMLElement>('[data-rail-item]')]
  if (!items.length) return
  e.preventDefault()
  const i = items.indexOf(document.activeElement as HTMLElement)
  const n = items.length
  const next = e.key === 'Home' ? 0 : e.key === 'End' ? n - 1 : e.key === 'ArrowDown' ? (i + 1) % n : i <= 0 ? n - 1 : i - 1
  items[next]?.focus()
}

/** A section icon and its popover, to the right of the rail, top-aligned with the icon and kept inside the window. */
function RailPopover({ id, pop, label, active, icon, children }: { id: string; pop: Pop; label: string; active?: boolean; icon: ReactNode; children: ReactNode }) {
  return (
    <Popover open={pop.open?.key === id} onOpenChange={(o) => !o && pop.dismiss(id)}>
      <PopoverTrigger asChild>
        <button
          type="button"
          className={RAIL_ITEM}
          aria-label={label}
          data-active={active || undefined}
          data-rail-section={id}
          onPointerEnter={pop.enter(id)}
          onPointerLeave={pop.leave}
          onClick={(e) => {
            // Ours, not Radix's toggle: a hover-opened popover stays open (pinned) on a click.
            e.preventDefault()
            pop.click(id, e.detail === 0 ? 'key' : 'click')
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
        className="flex max-h-(--radix-popover-content-available-height) w-72 flex-col overflow-y-auto rounded-xl p-1.5"
        onPointerEnter={(e) => e.pointerType === 'mouse' && pop.cancel()}
        onPointerLeave={pop.leave}
        onOpenAutoFocus={(e) => pop.via.current !== 'key' && e.preventDefault()}
        onEscapeKeyDown={() => (pop.escaped.current = id)}
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

function PopHeader({ icon, label, count, add }: { icon: ReactNode; label: string; count: number; add?: { label: string; run(): void } }) {
  return (
    <div className="mb-1 flex items-center gap-2 border-b border-border-subtle px-2 pt-1 pb-1.5">
      {icon}
      <span className="min-w-0 truncate text-sm font-semibold">{label}</span>
      <span className="shrink-0 text-xs text-fg-tertiary tabular-nums">{count}</span>
      {add && (
        <button
          type="button"
          data-rail-item
          aria-label={add.label}
          title={add.label}
          className="ml-auto flex size-7 shrink-0 items-center justify-center rounded-md text-fg-secondary outline-hidden hover:bg-accent hover:text-accent-foreground focus-visible:bg-accent pointer-coarse:size-9"
          onClick={add.run}
        >
          <Plus className="size-4" />
        </button>
      )}
    </div>
  )
}

const POP_ROW =
  'flex h-8 w-full items-center gap-2 rounded-md px-2 text-left text-[13px] text-fg-secondary outline-hidden hover:bg-accent hover:text-accent-foreground focus-visible:bg-accent focus-visible:text-accent-foreground data-[active=true]:font-medium data-[active=true]:text-foreground pointer-coarse:h-10'

/** One chat of a popover: the expanded row's glyphs and title, then how long since it last changed. */
function PopChat({ s, recent, subagents, active, onPick }: { s: SessionSummary; recent: boolean; subagents: readonly SessionSummary[] | undefined; active: boolean; onPick(): void }) {
  const title = useRowTitle(s.id, s.title, s.created_at)
  return (
    <Link
      to="/c/$session"
      params={{ session: s.id }}
      data-rail-item
      data-active={active || undefined}
      title={title}
      className={cn(POP_ROW, recent && 'opacity-70')}
      onClick={onPick}
    >
      <SessionActivity id={s.id} here={s.here} running={s.running} />
      <SessionPending id={s.id} />
      <SessionSubagents subagents={subagents} />
      <span className="min-w-0 flex-1 truncate">{title}</span>
      <span className="shrink-0 text-[11px] text-fg-tertiary tabular-nums">{ago(s.updated_at)}</span>
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

/**
 * The rail's lower half: one icon per section the expanded list shows (Active with a green dot while anything runs,
 * Pinned, each project in its own glyph and colour, Ungrouped), then Desks. Each opens a popover of its chats.
 */
export function RailSections({ active }: { active: string | undefined }) {
  const navigate = useNavigate()
  const { setOpen } = useSidebar()
  const { sessions, subs, activeSet, recent } = useSidebarSessions()
  const sections = useMemo(() => railSections(buildRows(sessions, [], undefined, false, activeSet, recent)), [sessions, activeSet, recent])
  const here = useMemo(() => rootOf(sessions, active), [sessions, active])
  const pop = useRailPopover()
  const showAll = (key: string) => {
    pop.close()
    revealSection(key)
    setOpen(true)
  }
  return (
    <>
      {sections.map((s) => {
        const add = s.kind === 'project' || s.kind === 'ungrouped' ? { label: newChatLabel(s.project), run: () => (pop.close(), openNewChat(navigate, s.project)) } : undefined
        return (
          <RailPopover
            key={s.key}
            id={s.key}
            pop={pop}
            label={`${s.label}, ${s.items.length} chat${s.items.length === 1 ? '' : 's'}`}
            active={here !== undefined && s.items.some((i) => i.session.id === here)}
            icon={
              <>
                {sectionIcon(s, 'size-[18px]')}
                {s.kind === 'active' && activeSet.size > 0 && (
                  <span className="absolute top-1.5 right-1.5 size-2 rounded-full bg-success ring-2 ring-sidebar" aria-label="running" />
                )}
              </>
            }
          >
            <PopHeader icon={sectionIcon(s, 'size-4')} label={s.label} count={s.items.length} add={add} />
            <ul>
              {s.items.slice(0, POPOVER_CAP).map(({ session, recent }) => (
                <li key={session.id}>
                  <PopChat s={session} recent={recent} subagents={subs.get(session.id)} active={session.id === here} onPick={pop.close} />
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
