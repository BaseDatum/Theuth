import { useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import { Archive, Brain, Check, Cog, Eye, type LucideIcon, MoreHorizontal, Network, PencilLine, Shrink, Timer, Trash2, Wrench } from 'lucide-react'
import { type ComponentType, memo, useState } from 'react'
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
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { HarvestIndicator, PtyIcon, ThinkingIndicator } from '@/components/chat/Indicators'
import { useSessionDrag } from '@/components/chat/Panes'
import { ChatPicker } from '@/components/notes/ChatPicker'
import { type AttachRef, chatName, toRef } from '@/lib/attach'
import { backgroundColors } from '@/lib/background-colors'
import { previewText } from '@/lib/channel-origin'
import { useProjects } from '@/components/chat/ProjectPicker'
import { parseSpoken } from '@/lib/injected'
import { parseNote } from '@/lib/voice-note'
import { Input } from '@/components/ui/input'
import { SidebarMenuAction, SidebarMenuButton } from '@/components/ui/sidebar'
import { touch } from '@/lib/hotkeys'
import { openMeeting, useMeetingForSession } from '@/lib/meeting-link'
import { canPopout, openPopout } from '@/lib/popout'
import { useApi, queryKeys } from '@/lib/rpc'
import type { SelectMode } from '@/lib/selection'
import { useRowSwipe } from '@/lib/use-row-swipe'
import { cn } from '@/lib/utils'
import type { SessionSummary } from "@/rpc/bindings/method.agent.output"
import { type Activity, type SessionSlice, shownActivity, useSessionField, useSessions } from '@/store/sessions'
import { routed, useEngineLabel } from '@/lib/owner'
import { useApprovals } from '@/store/approvals'
import { SidebarPopoutButton } from './SidebarPopoutButton'

/** One glyph per non-idle `Activity`: what the session is doing right now, readable at a glance in the list. */
const ACTIVITY: Record<Exclude<Activity, 'idle'>, [LucideIcon | ComponentType<{ className?: string; 'aria-label'?: string; role?: string }>, string]> = {
  thinking: [ThinkingIndicator, 'Thinking'],
  reasoning: [Brain, 'Reasoning'],
  streaming: [PencilLine, 'Replying'],
  tool: [Wrench, 'Running a tool'],
  compacting: [Shrink, 'Compacting context'],
  harvesting: [HarvestIndicator, 'Harvesting'],
  vision: [Eye, 'Inspecting images'],
}

/** Live activity glyph for a session row; nothing while idle. 183 W-7: a session owned elsewhere streams only while
 * focused, so for it `list_sessions.running` (refetched on replicated commits) gates the glyph; the slice's activity
 * only refines which one. `remoteRunning` is `undefined` for a row owned here. */
export const rowActivity = (shown: Activity, remoteRunning?: boolean): Activity =>
  remoteRunning === undefined ? shown : !remoteRunning ? 'idle' : shown === 'idle' ? 'thinking' : shown

/** The row's own turn glyph. A row owned here reads its slice, but `list_sessions.running` also counts: a turn already
 * running when this client (re)loaded has no `TurnStarted` in its slice (ACTIVE-TOP). */
export const turnActivity = (s: Pick<SessionSummary, 'here' | 'running'>, slice: SessionSlice | undefined): Activity =>
  rowActivity(shownActivity(slice?.activity, slice?.harvesting), s.here && !s.running ? undefined : s.running)

function SessionActivity({ id, here, running }: { id: string; here: boolean; running: boolean }) {
  const activity = useSessions((s) => turnActivity({ here, running }, s.sessions[id]))
  const status = useSessionField(id, 'statusText')
  if (activity === 'idle') return null
  const [Icon, word] = ACTIVITY[activity]
  // 212 §12: the agent's own status line, when it reported one, names the activity.
  const label = status || word
  // The lucide glyphs pulse; the loading-dev ones (thinking, harvesting) animate on their own.
  const glyph = (
    <Icon
      className={cn('size-3.5 shrink-0 text-info', activity !== 'thinking' && activity !== 'harvesting' && 'animate-pulse')}
      aria-label={label}
      role="img"
    />
  )
  return status ? (
    <span className="inline-flex shrink-0" title={status}>
      {glyph}
    </span>
  ) : (
    glyph
  )
}

/** The terminal glyph's tooltip: `open` terminals, `live` of them with a changing screen. */
const terminalLabel = (open: number, live: number): string =>
  open === 1 ? `Terminal ${live ? 'running' : 'idle'}` : !live ? `${open} terminals idle` : live === open ? `${open} terminals running` : `${open} terminals, ${live} running`

/** Pending-work glyphs, shown whether or not a turn runs: a cog per session with non-PTY background jobs still running, a terminal per
 * session with an open PTY (animated while a screen is changing, still while all are quiet: an idle terminal still wakes the agent on
 * its schedule), a timer (not pulsing: waiting, not working) per session with a reminder pending.
 * LIVE-GLYPHS (2026-10-07): the sets are seeded by `list_live_counts` and kept by `JobEvent`/`ReminderEvent`. */
function SessionPending({ id }: { id: string }) {
  const jobs = useSessionField(id, 'jobs')?.size ?? 0
  const ptys = useSessionField(id, 'ptys')
  const terminals = useSessionField(id, 'terminals')
  const open = new Set([...(terminals ?? []), ...(ptys ?? [])]).size
  const live = ptys?.size ?? 0
  const reminders = useSessionField(id, 'reminders')?.size ?? 0
  const pty = terminalLabel(open, live)
  return (
    <>
      {jobs > 0 && <Cog className={cn('size-3.5 shrink-0 animate-pulse', backgroundColors.jobs.text)} aria-label={`${jobs} background job${jobs === 1 ? '' : 's'}`} role="img" />}
      {open > 0 && (
        <PtyIcon live={live > 0} className={cn('size-3.5 shrink-0', backgroundColors.terminals.text)} aria-label={pty} role="img">
          <title>{pty}</title>
        </PtyIcon>
      )}
      {reminders > 0 && <Timer className={cn('size-3.5 shrink-0', backgroundColors.reminders.text)} aria-label={`${reminders} reminder${reminders === 1 ? '' : 's'} pending`} role="img" />}
    </>
  )
}

const NONE: SessionSummary[] = []

/** A descendant with a turn in flight: the live slice (`TurnStarted`…`TurnFinished`, every session is subscribed) or, before this client saw one, the `list_sessions` snapshot.
 * A watch-only child (141) has no turn of its own; the harness's state (`running` / `requires_action`, live from `ObservedChildChanged`, else the snapshot) is its live bit. */
const subagentRunning = (d: SessionSummary, slices: Record<string, SessionSlice>): boolean => {
  const observed = slices[d.id]?.observed ?? d.observed
  if (observed) return observed.state === 'running' || observed.state === 'requires_action'
  return !!(slices[d.id]?.running || d.running)
}

export const runningSubagents = (subagents: readonly SessionSummary[], slices: Record<string, SessionSlice>): number =>
  subagents.filter((d) => subagentRunning(d, slices)).length

/**
 * ACTIVE-TOP: which of `roots` the list floats into its `Active` section. A root counts when its own turn runs (a tool
 * call or an approval happens inside one), it holds a background job, open terminal (quiet included) or pending reminder, or a prompt
 * of its waits on the user (`prompting`); or when any descendant (`subs`) runs or holds any of those. The same
 * predicates as the row's glyphs, so the section and the glyphs cannot disagree.
 */
export function activeRoots(
  roots: readonly SessionSummary[],
  subs: ReadonlyMap<string, readonly SessionSummary[]>,
  slices: Record<string, SessionSlice>,
  prompting: ReadonlySet<string>,
): string[] {
  // A non-PTY background job, an open terminal or a pending reminder (what `SessionPending` shows), or a prompt.
  const busy = (d: SessionSummary) => {
    const s = slices[d.id]
    return (!!s && (s.jobs.size > 0 || s.terminals.size > 0 || s.ptys.size > 0 || s.reminders.size > 0)) || prompting.has(d.id)
  }
  return roots
    .filter((r) => turnActivity(r, slices[r.id]) !== 'idle' || busy(r) || (subs.get(r.id) ?? NONE).some((d) => subagentRunning(d, slices) || busy(d)))
    .map((r) => r.id)
}

/** Subagent glyph: shown while any descendant (any depth) is running; the tree below a selected row (or a desk) shows which. */
export function SessionSubagents({ subagents = NONE }: { subagents: readonly SessionSummary[] | undefined }) {
  const n = useSessions((s) => runningSubagents(subagents, s.sessions))
  const approval = useApprovals((s) => s.pending.some((p) => subagents.some((d) => d.id === p.session)))
  const observedApproval = useSessions((s) => subagents.some((d) => (s.sessions[d.id]?.observed ?? d.observed)?.state === 'requires_action'))
  if (n === 0) return null
  const label = `${n} subagent${n === 1 ? '' : 's'} running`
  return <Network className={cn('size-3.5 shrink-0 animate-pulse', backgroundColors[approval || observedApproval ? 'approval' : 'subagents'].text)} aria-label={label} role="img" />
}

/** Submenu body: the other known projects, then a new one (typed inline). Mounted only while the submenu is open. */
function ProjectItems({ current, onPick, onNew }: { current: string | null; onPick(p: string): void; onNew(): void }) {
  const others = useProjects().filter((p) => p !== current)
  return (
    <>
      {others.map((p) => (
        <DropdownMenuItem key={p} onSelect={() => onPick(p)}>
          {p}
        </DropdownMenuItem>
      ))}
      {others.length > 0 && <DropdownMenuSeparator />}
      <DropdownMenuItem onSelect={onNew}>New project…</DropdownMenuItem>
    </>
  )
}

/** Engine chip (68 §5): the owner's mesh name, muted, on a session owned elsewhere; hover names owner and epoch.
 * `prefix`: the chat header's `on ` (181 D4). 183 §12.1: `state` adds `· live` with a green dot (follow stream up), or `· offline`; `connected` adds nothing. */
export function EngineChip({ owner, epoch, className, prefix = '', state }: { owner: string; epoch: number; className?: string; prefix?: string; state?: 'live' | 'connected' | 'offline' }) {
  const label = useEngineLabel(owner)
  return (
    <span
      title={`owned by ${owner || '?'} · epoch ${epoch}`}
      className={cn('inline-flex shrink-0 items-center gap-1 rounded-[4px] bg-sidebar-accent px-1 text-[10px] leading-4 text-fg-tertiary', className)}
    >
      {state === 'live' && <span className="size-1.5 shrink-0 rounded-full bg-success" aria-hidden="true" />}
      {prefix}
      {label}
      {(state === 'live' || state === 'offline') && ` · ${state}`}
    </span>
  )
}

/**
 * One chat in the list: title (live from `TitleChanged`; the tooltip adds its tags), project pill (51 §3), active
 * highlight, hover `…` menu with pin, archive, inline rename, move to project (existing or new) and delete. ⌘-click /
 * ⇧-click / menu > Select feed the list's multi-selection. An archived row (51 §3) renders muted and its menu offers Unarchive.
 */
export const SessionItem = memo(function SessionItem({
  id,
  title,
  created,
  tags,
  pinned,
  archived,
  project,
  owner,
  epoch,
  here,
  running,
  endless,
  subagents,
  active,
  selected,
  selecting,
  select,
}: {
  id: string
  title: string
  created: string
  tags: string[]
  pinned: boolean
  archived: boolean
  project: string | null
  /** Owner engine id, its epoch and whether that is this engine (68 §5). */
  owner: string
  epoch: number
  here: boolean
  /** 183 W-7: `list_sessions.running` — for a row owned elsewhere, the replica's open turn. */
  running: boolean
  /** 137 §11.3: `SessionSummary.memory === 'endless'`; the ∞ glyph after the title. */
  endless: boolean
  /** Its subtree from `list_sessions` (all depths); absent when it has none. */
  subagents: readonly SessionSummary[] | undefined
  active: boolean
  selected: boolean
  selecting: boolean
  select(id: string, mode: SelectMode): void
}) {
  const api = useApi()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const live = useSessionField(id, 'title')
  const liveTags = useSessionField(id, 'tags') ?? tags
  const raw = live || chatName({ title, created_at: created })
  // A session opened by a voice note or a talk is titled from its input; the `[voice note, N s]` / `[spoken, N s]` marker is not a title (45 §7.4),
  // nor is a Slack marker line (172).
  const shown = parseNote(raw)?.body || parseSpoken(raw)?.text || previewText(raw)
  const tip = liveTags.length ? `${shown} · ${liveTags.join(', ')}` : shown
  const drag = useSessionDrag(id, shown)
  // Inline editor: the title, or the name of a new project.
  const [editing, setEditing] = useState<{ field: 'title' | 'project'; value: string } | null>(null)
  const [confirming, setConfirming] = useState(false)
  const [menuOpen, setMenuOpen] = useState(false)
  const [attaching, setAttaching] = useState(false)
  const meeting = useMeetingForSession(id)
  /** 66 §7.8: this chat as a reference — the list row's record when the sessions query has it. */
  const asRef = (): AttachRef => {
    const row = qc.getQueryData<SessionSummary[]>(queryKeys.sessions)?.find((x) => x.id === id)
    return row ? toRef({ kind: 'session', session: row }) : { kind: 'session', id, title: shown, meta: `chat · ${project ?? 'no project'}${archived ? ' · archived' : ''}` }
  }

  const remove = async () => {
    try {
      // 183 §4.4: a parked delete (owner offline) leaves the chat open — `routed` resolves undefined.
      const done = await routed(id, async () => (await api.delete_session(id), true))
      if (done && active) void navigate({ to: '/' })
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }

  const rename = async (value: string) => {
    setEditing(null)
    if (!value.trim() || value === shown) return
    try {
      await routed(id, () => api.rename_session(id, value.trim()))
      void qc.invalidateQueries({ queryKey: queryKeys.sessions })
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }

  const move = async (value: string | null) => {
    setEditing(null)
    const next = value?.trim() || null
    if (next === project) return
    try {
      await routed(id, () => api.set_session_project(id, next))
      void qc.invalidateQueries({ queryKey: queryKeys.sessions })
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }

  const pin = async () => {
    try {
      await routed(id, () => api.pin_session(id, !pinned))
      void qc.invalidateQueries({ queryKey: queryKeys.sessions })
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }

  // Touch: swipe left (right to left) archives; swipe right opens the same Delete confirmation as the menu.
  const swipe = useRowSwipe({ onSwipeLeft: () => void archive(), onSwipeRight: () => setConfirming(true) })

  const archive = async () => {
    try {
      await routed(id, () => api.archive_session(id, !archived))
      void qc.invalidateQueries({ queryKey: queryKeys.sessions })
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }

  const take = async () => {
    try {
      await api.session_take(id, 'user')
      void qc.invalidateQueries({ queryKey: queryKeys.sessions })
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }

  return (
    <div className="group/menu-item group/popout-row relative">
      {swipe.dx !== 0 && (
        <div aria-hidden className={cn('absolute inset-0 flex items-center rounded-[10px] px-3', swipe.dx > 0 ? 'justify-start bg-danger-bg text-danger' : 'justify-end bg-warning-bg text-warning')}>
          {swipe.dx > 0 ? <Trash2 className="size-4" /> : <Archive className="size-4" />}
        </div>
      )}
      {editing !== null ? (
        <Input
          autoFocus
          placeholder={editing.field === 'project' ? 'New project' : undefined}
          maxLength={editing.field === 'project' ? 128 : undefined}
          value={editing.value}
          onChange={(e) => setEditing({ field: editing.field, value: e.target.value })}
          onBlur={() => setEditing(null)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') void (editing.field === 'title' ? rename(editing.value) : move(editing.value))
            if (e.key === 'Escape') setEditing(null)
          }}
          className="h-9 rounded-[10px] text-sm"
        />
      ) : (
        <div {...swipe.handlers} style={swipe.style} className="relative rounded-[10px] bg-sidebar">
          <SidebarMenuButton
            asChild
            isActive={active}
            aria-selected={selected}
            className={cn('h-9 rounded-[10px] px-2.5', canPopout() && 'pointer-fine:pr-16!', selected && 'bg-sidebar-accent', archived && 'text-fg-tertiary')}
          >
            <Link
              to="/c/$session"
              params={{ session: id }}
              title={tip}
              {...drag}
              onContextMenu={(e) => {
                // A long press that became a drag is eaten by `drag.onContextMenuCapture` before this runs.
                e.preventDefault()
                setMenuOpen(true)
              }}
              onClick={(e) => {
                if (!touch() && (selecting || e.metaKey || e.shiftKey)) {
                  e.preventDefault()
                  select(id, e.shiftKey ? 'range' : 'toggle')
                } else {
                  select(id, 'clear')
                }
              }}
            >
              {selecting && (
                <span
                  className={cn(
                    'flex size-4 shrink-0 items-center justify-center rounded-[4px] border border-fg-tertiary',
                    selected && 'border-foreground bg-foreground text-background',
                  )}
                >
                  {selected && <Check className="size-3 stroke-[3]" />}
                </span>
              )}
              <SessionActivity id={id} here={here} running={running} />
              <SessionPending id={id} />
              <SessionSubagents subagents={subagents} />
              <span className="truncate">{shown}</span>
              {endless && (
                <span role="img" aria-label="Fractal context" className="shrink-0 text-sm leading-4 text-fg-tertiary">
                  ∞
                </span>
              )}
              {!here && <EngineChip owner={owner} epoch={epoch} className="ml-auto" />}
            </Link>
          </SidebarMenuButton>
          <SidebarPopoutButton target={{ kind: 'chat', session: id }} className="top-2 right-8" />
          <DropdownMenu open={menuOpen} onOpenChange={setMenuOpen}>
            <DropdownMenuTrigger asChild>
              <SidebarMenuAction showOnHover className="top-2 right-2" aria-label="Chat options">
                <MoreHorizontal />
              </SidebarMenuAction>
            </DropdownMenuTrigger>
            <DropdownMenuContent side="right" align="start" className="w-48">
              <DropdownMenuItem onSelect={() => setEditing({ field: 'title', value: shown })}>Rename</DropdownMenuItem>
              {canPopout() && <DropdownMenuItem onSelect={() => void openPopout({ kind: 'chat', session: id })}>Open in new window</DropdownMenuItem>}
              {meeting && (
                <>
                  <DropdownMenuItem onClick={(e) => openMeeting(meeting, e.altKey, () => void navigate({ to: '/meetings/$id', params: { id: meeting } }))}>
                    Open meeting
                  </DropdownMenuItem>
                  {canPopout() && <DropdownMenuItem onSelect={() => void openPopout({ kind: 'meeting', meeting })}>Open meeting in new window</DropdownMenuItem>}
                </>
              )}
              <DropdownMenuItem onSelect={() => void pin()}>{pinned ? 'Unpin' : 'Pin'}</DropdownMenuItem>
              <DropdownMenuItem onSelect={() => void archive()}>{archived ? 'Unarchive' : 'Archive'}</DropdownMenuItem>
              <DropdownMenuItem
                disabled={here}
                title={here ? 'This session already runs on this node' : undefined}
                onSelect={() => void take()}
              >
                Continue here
              </DropdownMenuItem>
              <DropdownMenuSub>
                <DropdownMenuSubTrigger>Move to project</DropdownMenuSubTrigger>
                <DropdownMenuSubContent className="w-48">
                  <ProjectItems current={project} onPick={(p) => void move(p)} onNew={() => setEditing({ field: 'project', value: '' })} />
                </DropdownMenuSubContent>
              </DropdownMenuSub>
              {project && <DropdownMenuItem onSelect={() => void move(null)}>Remove from project</DropdownMenuItem>}
              <DropdownMenuItem onSelect={() => select(id, 'toggle')}>
                {selected ? 'Deselect' : 'Select'}
              </DropdownMenuItem>
              <DropdownMenuItem onSelect={() => setAttaching(true)}>Attach to chat…</DropdownMenuItem>
              <DropdownMenuItem onSelect={() => void navigator.clipboard.writeText(id)}>
                Copy id
              </DropdownMenuItem>
              <DropdownMenuItem variant="destructive" onSelect={() => setConfirming(true)}>
                Delete
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
          {attaching && <ChatPicker open onOpenChange={setAttaching} item={asRef()} />}
          <AlertDialog open={confirming} onOpenChange={setConfirming}>
            <AlertDialogContent>
              <AlertDialogHeader>
                <AlertDialogTitle>Delete chat?</AlertDialogTitle>
                <AlertDialogDescription>
                  “{shown}” and any subagent chats under it are removed for good.
                </AlertDialogDescription>
              </AlertDialogHeader>
              <AlertDialogFooter>
                <AlertDialogCancel>Cancel</AlertDialogCancel>
                <AlertDialogAction onClick={() => void remove()}>Delete</AlertDialogAction>
              </AlertDialogFooter>
            </AlertDialogContent>
          </AlertDialog>
        </div>
      )}
    </div>
  )
})
