import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { MoreHorizontal } from 'lucide-react'
import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Input } from '@/components/ui/input'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { age } from '@/components/tasks/helpers'
import { useAgents, useMesh } from '@/components/tasks/queries'
import { openMeeting, useMeetingForSession } from '@/lib/meeting-link'
import { usePanes } from '@/lib/panes'
import { canPopout, openPopout } from '@/lib/popout'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { AgentSummary } from '@/rpc/bindings/AgentSummary'
import type { CompactStyle } from '@/rpc/bindings/CompactStyle'
import type { MeshStatus } from '@/rpc/bindings/MeshStatus'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import { useSessionField, useSessions } from '@/store/sessions'
import { type Ctx, openSurface, run, settings, type Surface, surfaceOf } from './slash'
import { PopoutTerminalButton } from './TerminalViewer'

const STYLES: CompactStyle[] = ['summary', 'cut', 'snap']

/** One row of Move to ▸ (181 D1). */
export type MoveTarget = { id: string; name: string; online: boolean; detail: string }

/** 181 D1: every paired engine but this one, by name; an offline one says for how long (and is disabled); an online one
 * says what stays behind when `running` jobs are live here. */
export function moveTargets(mesh: Pick<MeshStatus, 'self_id' | 'peers'> | undefined, running: number, now = Date.now()): MoveTarget[] {
  const jobs = running === 0 ? '' : `${running} running ${running === 1 ? 'job stays' : 'jobs stay'} here`
  return (mesh?.peers ?? [])
    .filter((p) => p.id !== mesh?.self_id)
    .map((p) => {
      const seen = Number(p.last_seen_ms)
      return {
        id: p.id,
        name: p.name || p.id.slice(0, 8),
        online: p.reachable,
        detail: p.reachable ? jobs : seen > 0 ? `offline ${age(seen, now)}` : 'offline',
      }
    })
}

type MoveRow = Pick<SessionSummary, 'id' | 'parent' | 'agent' | 'desk'>

/** 181 fix round: why this session cannot move, in the engine's words (`move_to`), or `null`. A paused agent's tree is
 * found by walking `parent` to the root and matching that desk in `agents_list`. */
export function moveBlocked(row: MoveRow | undefined, rows: readonly MoveRow[], agents: readonly Pick<AgentSummary, 'status' | 'desk' | 'file'>[]): string | null {
  if (!row) return null
  let root = row
  for (let up = rows.find((r) => r.id === root.parent); up; up = rows.find((r) => r.id === root.parent)) root = up
  const paused = agents.find((a) => a.status === 'paused' && (a.desk === root.id || a.desk === row.id))
  if (paused) return `${paused.file.name} is paused — un-pause it before moving`
  if (row.agent) return `runs the ${row.agent} harness as a process here, so it cannot move`
  if (row.desk) return `${row.desk}'s desk stays on the engine that runs its agent`
  return null
}

/** Rename a session; `rename_session` fires `TitleChanged`, which updates the slice and the list. */
export function RenameDialog({
  session,
  open,
  onOpenChange,
}: {
  session: string
  open: boolean
  onOpenChange(o: boolean): void
}) {
  const api = useApi()
  const qc = useQueryClient()
  const title = useSessionField(session, 'title')
  const [value, setValue] = useState('')
  useEffect(() => {
    if (open) setValue(title ?? '')
  }, [open, title])
  const save = async () => {
    onOpenChange(false)
    try {
      await api.rename_session(session, value.trim())
      void qc.invalidateQueries({ queryKey: queryKeys.sessions })
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-sm">
        <DialogHeader>
          <DialogTitle>Rename chat</DialogTitle>
        </DialogHeader>
        <Input
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && value.trim() && void save()}
          autoFocus
        />
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button disabled={!value.trim()} onClick={() => void save()}>
            Save
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function PromptSheet({
  session,
  open,
  onOpenChange,
}: {
  session: string
  open: boolean
  onOpenChange(o: boolean): void
}) {
  const api = useApi()
  const { data, error } = useQuery({
    queryKey: queryKeys.prompt(session),
    queryFn: () => api.get_prompt(session),
    enabled: open,
  })
  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent className="w-full sm:w-[640px] sm:max-w-[640px]">
        <SheetHeader>
          <SheetTitle>System prompt</SheetTitle>
          <SheetDescription>What the model sees at the top of this session.</SheetDescription>
        </SheetHeader>
        <pre className="mx-4 mb-4 min-h-0 flex-1 overflow-auto rounded-xl bg-layer-2 p-4 font-mono text-xs leading-5 whitespace-pre-wrap">
          {error ? (error instanceof Error ? error.message : String(error)) : (data ?? 'Loading…')}
        </pre>
      </SheetContent>
    </Sheet>
  )
}

function JobsSheet({
  session,
  open,
  onOpenChange,
}: {
  session: string
  open: boolean
  onOpenChange(o: boolean): void
}) {
  const api = useApi()
  const key = queryKeys.jobs(session)
  const { data: jobs = [], refetch } = useQuery({
    queryKey: key,
    queryFn: () => api.list_jobs(session),
    enabled: open,
    refetchInterval: open ? 2000 : false,
  })
  const kill = async (id: string) => {
    try {
      await api.cancel_job(session, id)
      toast(`killed ${id}`)
      void refetch()
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }
  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent className="w-full sm:w-[640px] sm:max-w-[640px]">
        <SheetHeader>
          <SheetTitle>Background jobs</SheetTitle>
          <SheetDescription>
            {jobs.length === 0 ? 'No jobs in this session.' : `${jobs.length} in this session`}
          </SheetDescription>
        </SheetHeader>
        <div className="mx-4 mb-4 min-h-0 flex-1 overflow-auto text-sm">
          <table className="w-full">
            <thead className="text-left text-xs text-fg-tertiary">
              <tr>
                <th className="py-1 font-medium">id</th>
                <th className="py-1 font-medium">command</th>
                <th className="py-1 font-medium">state</th>
                <th className="py-1 font-medium">secs</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {jobs.map((j) => (
                <tr key={j.id} className="border-t border-border-subtle align-top">
                  <td className="py-2 font-mono text-xs">{j.id}</td>
                  <td className="max-w-64 truncate py-2 font-mono text-xs" title={j.command}>
                    {j.name ? `${j.name} · ` : ''}
                    {j.command}
                  </td>
                  {/* 109 §8.4: a job that died with the engine; its log is still readable. */}
                  <td className={j.state === 'lost' ? 'py-2 text-warning' : 'py-2'}>{j.state === 'lost' ? 'lost (engine restarted)' : j.state}</td>
                  <td className="py-2 tabular-nums">{Number(j.secs)}</td>
                  <td className="py-1 text-right whitespace-nowrap">
                    {j.pty && <PopoutTerminalButton session={j.session} job={j.id} className="size-7" />}
                    {j.state === 'running' && (
                      <Button size="xs" variant="outline" onClick={() => void kill(j.id)}>
                        Kill
                      </Button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </SheetContent>
    </Sheet>
  )
}

/** The `…` menu of the open session; every action is the slash command the TUI would run. */
export function SessionMenu({ session }: { session: string }) {
  const api = useApi()
  const navigate = useNavigate()
  const parent = useSessionField(session, 'parent')
  const undone = useSessionField(session, 'undone') ?? 0
  const liveTags = useSessionField(session, 'tags')
  // Tags: live from `TitleChanged`, else the stored set from the session list; the owner row (68 §5) is always the list's.
  const sessions = useQuery({ queryKey: queryKeys.sessions, queryFn: () => api.list_sessions() })
  const row = sessions.data?.find((s) => s.id === session)
  const tags = liveTags ?? row?.tags ?? []
  const here = row?.here ?? true
  // 70 POPOUT-2 item 3: the meeting this chat belongs to (its own session); no meeting, no items.
  const meeting = useMeetingForSession(session)
  const [rename, setRename] = useState(false)
  const [sheet, setSheet] = useState<Surface | null>(null)
  // 181 D1: the paired engines and this session's live jobs, read while the menu is open.
  const [open, setOpen] = useState(false)
  const mesh = useMesh()
  const agents = useAgents()
  const jobs = useQuery({ queryKey: queryKeys.jobs(session), queryFn: () => api.list_jobs(session), enabled: open && here })
  const targets = here ? moveTargets(mesh.data, (jobs.data ?? []).filter((j) => j.state === 'running').length) : []
  const blocked = moveBlocked(row, sessions.data ?? [], agents.data ?? [])
  const queryClient = useQueryClient()
  const move = async (t: MoveTarget) => {
    try {
      await api.session_move(session, t.id)
      useSessions.getState().setMoved(session, t.id)
      toast(`Moved to ${t.name}`)
    } catch (e) {
      // The engine's words: offline, refused ("… It continues here."), or no answer yet (the move may still land).
      toast.error(e instanceof Error ? e.message : String(e))
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions })
    }
  }

  const focused = usePanes((s) => s.focused === session)
  useEffect(() => {
    if (!focused) return
    const on = (e: Event) => {
      const s = surfaceOf(e)
      if (s === 'prompt' || s === 'jobs') setSheet(s)
    }
    window.addEventListener('theuth:open', on)
    return () => window.removeEventListener('theuth:open', on)
  }, [focused])

  const ctx: Ctx = {
    api,
    session,
    parent,
    navigate,
    async reload() {
      useSessions
        .getState()
        .loadHistory(session, await api.load_session(session, null, 60), 'replace')
    },
  }
  const cmd = (line: string) => () =>
    void run(line, ctx).catch((e: unknown) =>
      toast.error(e instanceof Error ? e.message : String(e)),
    )

  return (
    <>
      <DropdownMenu
        onOpenChange={(o) => {
          setOpen(o)
          if (o) void mesh.refetch()
        }}
      >
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            className="size-9 rounded-lg text-fg-secondary"
            aria-label="Session menu"
          >
            <MoreHorizontal className="size-5" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-56">
          {tags.length > 0 && (
            <>
              <DropdownMenuLabel className="flex flex-wrap gap-1 py-1.5">
                {tags.map((t) => (
                  <span key={t} className="rounded-full border border-border-subtle px-1.5 text-[10px] font-normal leading-4 text-fg-tertiary">
                    {t}
                  </span>
                ))}
              </DropdownMenuLabel>
              <DropdownMenuSeparator />
            </>
          )}
          <DropdownMenuItem onSelect={() => setRename(true)}>Rename</DropdownMenuItem>
          <DropdownMenuItem onSelect={() => void navigator.clipboard.writeText(session)}>
            Copy id
          </DropdownMenuItem>
          {parent && <DropdownMenuItem onSelect={cmd('/parent')}>Open parent</DropdownMenuItem>}
          {meeting && (
            <>
              <DropdownMenuItem onClick={(e) => openMeeting(meeting, e.altKey, () => void navigate({ to: '/meetings/$id', params: { id: meeting } }))}>
                Open meeting
              </DropdownMenuItem>
              {canPopout() && <DropdownMenuItem onSelect={() => void openPopout({ kind: 'meeting', meeting })}>Open meeting in new window</DropdownMenuItem>}
            </>
          )}
          <DropdownMenuItem
            disabled={here}
            title={here ? 'This session already runs on this node' : undefined}
            onSelect={cmd('/take')}
          >
            Continue here
          </DropdownMenuItem>
          {targets.length > 0 && blocked && (
            <DropdownMenuItem disabled className="flex-col items-start gap-0">
              Move to
              <span className="text-[11px] text-fg-tertiary">{blocked}</span>
            </DropdownMenuItem>
          )}
          {targets.length > 0 && !blocked && (
            <DropdownMenuSub>
              <DropdownMenuSubTrigger>Move to</DropdownMenuSubTrigger>
              <DropdownMenuSubContent className="w-60">
                {targets.map((t) => (
                  <DropdownMenuItem key={t.id} disabled={!t.online} onSelect={() => void move(t)} className="flex-col items-start gap-0">
                    <span className="flex items-center gap-1.5">
                      <span className={cn('size-1.5 rounded-full', t.online ? 'bg-success' : 'bg-fg-tertiary')} />
                      Move to {t.name}
                    </span>
                    {t.detail && <span className="pl-3 text-[11px] text-fg-tertiary">{t.detail}</span>}
                  </DropdownMenuItem>
                ))}
              </DropdownMenuSubContent>
            </DropdownMenuSub>
          )}
          <DropdownMenuSeparator />
          <DropdownMenuSub>
            <DropdownMenuSubTrigger>Compact</DropdownMenuSubTrigger>
            <DropdownMenuSubContent>
              {STYLES.map((s) => (
                <DropdownMenuItem key={s} onSelect={cmd(`/compact ${s}`)}>
                  {s}
                </DropdownMenuItem>
              ))}
            </DropdownMenuSubContent>
          </DropdownMenuSub>
          <DropdownMenuItem onSelect={cmd('/clear')}>Clear</DropdownMenuItem>
          <DropdownMenuItem onSelect={cmd('/shake')}>Shake context…</DropdownMenuItem>
          <DropdownMenuItem onSelect={cmd('/undo')}>Undo</DropdownMenuItem>
          <DropdownMenuItem disabled={undone === 0} onSelect={cmd('/redo')}>
            Redo
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem onSelect={cmd('/knowledge harvest')}>
            Harvest knowledge
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={() => openSurface('prompt')}>Show prompt</DropdownMenuItem>
          <DropdownMenuItem onSelect={() => openSurface('jobs')}>Jobs</DropdownMenuItem>
          <DropdownMenuItem onSelect={() => void settings(navigate, 'communication', session)}>
            Communication &amp; peers…
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <RenameDialog session={session} open={rename} onOpenChange={setRename} />
      <PromptSheet
        session={session}
        open={sheet === 'prompt'}
        onOpenChange={(o) => !o && setSheet(null)}
      />
      <JobsSheet
        session={session}
        open={sheet === 'jobs'}
        onOpenChange={(o) => !o && setSheet(null)}
      />
    </>
  )
}
