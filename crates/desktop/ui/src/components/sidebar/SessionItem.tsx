import { useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import { Archive, Brain, Check, Cog, Eye, LoaderCircle, type LucideIcon, Mic, MoreHorizontal, PencilLine, Shrink, SquareTerminal, Wrench } from 'lucide-react'
import { memo, useState } from 'react'
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
import { useSessionDrag } from '@/components/chat/Panes'
import { useProjects } from '@/components/chat/ProjectPicker'
import { parseNote } from '@/lib/voice-note'
import { Input } from '@/components/ui/input'
import { SidebarMenuAction, SidebarMenuButton } from '@/components/ui/sidebar'
import { touch } from '@/lib/hotkeys'
import { useApi, queryKeys } from '@/lib/rpc'
import type { SelectMode } from '@/lib/selection'
import { cn } from '@/lib/utils'
import { type Activity, shownActivity, useSessionField } from '@/store/sessions'

/** One glyph per non-idle `Activity`: what the session is doing right now, readable at a glance in the list. */
const ACTIVITY: Record<Exclude<Activity, 'idle'>, [LucideIcon, string]> = {
  thinking: [LoaderCircle, 'Thinking'],
  reasoning: [Brain, 'Reasoning'],
  streaming: [PencilLine, 'Replying'],
  tool: [Wrench, 'Running a tool'],
  compacting: [Shrink, 'Compacting context'],
  harvesting: [Archive, 'Harvesting'],
  vision: [Eye, 'Inspecting images'],
}

/** Live activity glyph for a session row; nothing while idle. */
function SessionActivity({ id }: { id: string }) {
  const activity = shownActivity(useSessionField(id, 'activity'), useSessionField(id, 'harvesting'))
  if (activity === 'idle') return null
  const [Icon, label] = ACTIVITY[activity]
  return (
    <Icon
      className={cn('size-3.5 shrink-0 text-info', activity === 'thinking' ? 'animate-spin' : 'animate-pulse')}
      aria-label={label}
      role="img"
    />
  )
}

/** Live-job glyphs: a cog per session with non-PTY background jobs still running, a terminal per session with a PTY whose screen is still changing. */
function SessionJobs({ id }: { id: string }) {
  const jobs = useSessionField(id, 'jobs')?.size ?? 0
  const ptys = useSessionField(id, 'ptys')?.size ?? 0
  return (
    <>
      {jobs > 0 && <Cog className="size-3.5 shrink-0 animate-pulse text-info" aria-label={`${jobs} background job${jobs === 1 ? '' : 's'}`} role="img" />}
      {ptys > 0 && <SquareTerminal className="size-3.5 shrink-0 animate-pulse text-info" aria-label={`${ptys} live terminal${ptys === 1 ? '' : 's'}`} role="img" />}
    </>
  )
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

/**
 * One chat in the list: title (live from `TitleChanged`), project pill (51 §3), active highlight, hover `…` menu with
 * pin, archive, inline rename, move to project (existing or new) and delete. ⌘-click / ⇧-click / menu > Select feed the
 * list's multi-selection. An archived row (51 §3) renders muted and its menu offers Unarchive.
 */
export const SessionItem = memo(function SessionItem({
  id,
  title,
  pinned,
  archived,
  project,
  active,
  selected,
  selecting,
  select,
}: {
  id: string
  title: string
  pinned: boolean
  archived: boolean
  project: string | null
  active: boolean
  selected: boolean
  selecting: boolean
  select(id: string, mode: SelectMode): void
}) {
  const api = useApi()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const live = useSessionField(id, 'title')
  const raw = live || title || id.slice(0, 8)
  // A session opened by a voice note is titled from its input; the `[voice note, N s]` marker is not a title (45 §7.4).
  const shown = parseNote(raw)?.body || raw
  const drag = useSessionDrag(id, shown)
  // Inline editor: the title, or the name of a new project.
  const [editing, setEditing] = useState<{ field: 'title' | 'project'; value: string } | null>(null)
  const [confirming, setConfirming] = useState(false)

  const remove = async () => {
    try {
      await api.delete_session(id)
      if (active) void navigate({ to: '/' })
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }

  const rename = async (value: string) => {
    setEditing(null)
    if (!value.trim() || value === shown) return
    try {
      await api.rename_session(id, value.trim())
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
      await api.set_session_project(id, next)
      void qc.invalidateQueries({ queryKey: queryKeys.sessions })
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }

  const pin = async () => {
    try {
      await api.pin_session(id, !pinned)
      void qc.invalidateQueries({ queryKey: queryKeys.sessions })
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }

  const archive = async () => {
    try {
      await api.archive_session(id, !archived)
      void qc.invalidateQueries({ queryKey: queryKeys.sessions })
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }

  return (
    <div className="group/menu-item relative">
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
        <>
          <SidebarMenuButton
            asChild
            isActive={active}
            aria-selected={selected}
            className={cn('h-9 rounded-[10px] px-2.5', selected && 'bg-sidebar-accent', archived && 'text-fg-tertiary')}
          >
            <Link
              to="/c/$session"
              params={{ session: id }}
              title={shown}
              {...drag}
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
              <SessionActivity id={id} />
              <SessionJobs id={id} />
              <span className="truncate">{shown}</span>
              {project === 'meetings' ? (
                // 53 / 01 §0.1: a meeting's session; the sidebar filters on `project`.
                <Mic className="ml-auto size-3.5 shrink-0 text-fg-tertiary" aria-label="Meeting" />
              ) : (
                project && (
                  <span className="ml-auto shrink-0 rounded-full border border-border-subtle px-1.5 text-[10px] leading-4 text-fg-tertiary">
                    {project}
                  </span>
                )
              )}
            </Link>
          </SidebarMenuButton>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <SidebarMenuAction showOnHover className="top-2 right-2" aria-label="Chat options">
                <MoreHorizontal />
              </SidebarMenuAction>
            </DropdownMenuTrigger>
            <DropdownMenuContent side="right" align="start" className="w-48">
              <DropdownMenuItem onSelect={() => setEditing({ field: 'title', value: shown })}>Rename</DropdownMenuItem>
              <DropdownMenuItem onSelect={() => void pin()}>{pinned ? 'Unpin' : 'Pin'}</DropdownMenuItem>
              <DropdownMenuItem onSelect={() => void archive()}>{archived ? 'Unarchive' : 'Archive'}</DropdownMenuItem>
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
              <DropdownMenuItem onSelect={() => void navigator.clipboard.writeText(id)}>
                Copy id
              </DropdownMenuItem>
              <DropdownMenuItem variant="destructive" onSelect={() => setConfirming(true)}>
                Delete
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
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
        </>
      )}
    </div>
  )
})
