import { Capability, useQuerySupport } from '@/rpc/capabilities'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { MoreHorizontal } from 'lucide-react'
import { Fragment, useEffect, useState } from 'react'
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
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
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
import { useDrafts } from '@/lib/drafts'
import { usePanes } from '@/lib/panes'
import { tileable } from '@/lib/tiles'
import { openJobsTile, openPromptTile } from '@/components/tiles/open'
import { canPopout, openPopout } from '@/lib/popout'
import { engineLabel, ownerUnreachable, parkIfOffline, routed, useOwnerOffline } from '@/lib/owner'
import { RpcError } from '@/rpc/client'
import { chatName } from '@/lib/attach'
import { ShareDialog, SharingItems } from '@/components/settings/Contacts'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { AgentSummary } from "@/rpc/bindings/method.roster.output"
import type { ApprovalsState } from "@/rpc/bindings/method.agent.output"
import type { CommunicationState } from "@/rpc/bindings/method.agent.output"
import type { CompactStyle } from "@/rpc/bindings/method.agent.output"
import type { MeshStatus } from "@/rpc/bindings/method.nodes.output"
import type { SessionSummary } from "@/rpc/bindings/method.agent.output"
import { useSessionField, useSessions } from '@/store/sessions'
import { useModels } from './ModelMenu'
import { useMessagingSheet } from './messaging-sheet'
import { useToolsSheet } from './tools-sheet'
import { type Ctx, openSurface, run, type Surface, surfaceOf } from './slash'
import { JobsSheet } from './JobsSheet'
import { shareRequirements } from './requirements'

/** One row of the ⋯ menu's settings block (183 §12.2, mock panel 3), in order; `move` is 181's Move to ▸. */
export type SettingsRow =
  | { id: 'header' | 'offline' | 'separator' | 'move'; label: string }
  | { id: 'approvals'; label: string; value: 'on' | 'off' | 'default' | undefined; options: { value: 'on' | 'off' | 'default'; label: string }[]; disabled: boolean; note?: string }
  | { id: 'messaging'; label: string; checked: boolean; disabled: boolean }
  | { id: 'model' | 'continue'; label: string; disabled: boolean }

/**
 * 183 §12.2: the session's own settings, then where it runs. `owner` is the owner's label for a session owned
 * elsewhere (`undefined` here); `offline` greys every setter with the line that says why (§4.4, edge 13).
 */
export function settingsRows(o: {
  owner?: string
  self: string
  offline: boolean
  approvals?: Pick<ApprovalsState, 'global'> & { session?: boolean; pin?: ApprovalsState['pin'] | null }
  /** 183 R1 (S7): the owner is pre-183 and answered `get_approvals` -32601; the row says so instead of an error. */
  oldOwner?: boolean
  messaging?: Pick<CommunicationState, 'effective'>
  model: string
  /** The session's effort (`null` = the model's default); the model row names it, the picker it opens has the chips. */
  effort?: string | null
}): SettingsRow[] {
  const off = o.offline && o.owner !== undefined
  // §12.2 (R1): what inherit gives is the EFFECTIVE value (`session`: an ancestor's pin can differ from the machine's switch)
  // when this session has no pin of its own; with a pin set, the machine's switch is the best the reader tells us.
  const inherited = o.approvals ? (o.approvals.pin == null ? (o.approvals.session ?? o.approvals.global) : o.approvals.global) : undefined
  const machine = inherited === undefined ? '' : ` (${inherited ? 'on' : 'off'})`
  return [
    ...(o.owner === undefined ? [] : [{ id: 'header' as const, label: `Settings for this session on ${o.owner}` }]),
    ...(off ? [{ id: 'offline' as const, label: `${o.owner} is offline · Continue here` }] : []),
    {
      id: 'approvals',
      label: 'Approvals: Ask before risky tools',
      // W-4 tri-state: the row's own pin (`ApprovalsState.pin`; the engine omits it, older builds sent null) — none inherits.
      value: o.approvals && (o.approvals.pin == null ? 'default' : o.approvals.pin ? 'on' : 'off'),
      options: [
        { value: 'on', label: 'On' },
        { value: 'off', label: 'Off' },
        { value: 'default', label: `${o.owner === undefined ? 'Machine' : o.owner} default${machine}` },
      ],
      disabled: off || !o.approvals || !!o.oldOwner,
      note: o.oldOwner && o.owner !== undefined ? `${o.owner} needs an update` : undefined,
    },
    { id: 'messaging', label: 'Agent messaging', checked: o.messaging?.effective ?? false, disabled: off || !o.messaging },
    // §12.2, 2026-11-11 (Sky): Model ›, with the effort, opens the standard picker; its effort chips replaced the Effort › submenu.
    { id: 'model', label: `Model: ${o.model}${o.effort ? ` · ${o.effort}` : ''}`, disabled: off },
    { id: 'separator', label: '' },
    { id: 'continue', label: o.owner === undefined ? 'Continue here' : `Continue here (move to ${o.self})`, disabled: o.owner === undefined },
    { id: 'move', label: 'Move to' },
  ]
}

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
      // 183 §4.4: a rename is the owner's act; with it gone the call waits behind the card.
      await routed(session, () => api.rename_session(session, value.trim()))
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
  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent className="w-full sm:w-[640px] sm:max-w-[640px]">
        <SheetHeader>
          <SheetTitle>System prompt</SheetTitle>
          <SheetDescription>{PROMPT_NOTE}</SheetDescription>
        </SheetHeader>
        <PromptBody session={session} />
      </SheetContent>
    </Sheet>
  )
}

export const PROMPT_NOTE = 'What the model sees at the top of this session.'

/** The session's system prompt, in `PromptSheet` or a `prompt` tile; read while mounted. */
export function PromptBody({ session, className }: { session: string; className?: string }) {
  const canQuery1 = useQuerySupport(["get_prompt"])
  const api = useApi()
  const { data, error } = useQuery({
    queryKey: queryKeys.prompt(session),
    queryFn: () => api.get_prompt(session),
    enabled: canQuery1,
  })
  return (
    <pre className={cn('mx-4 mb-4 min-h-0 flex-1 overflow-auto rounded-xl bg-layer-2 p-4 font-mono text-xs leading-5 whitespace-pre-wrap', className)}>
      {error ? (error instanceof Error ? error.message : String(error)) : (data ?? 'Loading…')}
    </pre>
  )
}


/** The `…` menu of the open session; every action is the slash command the TUI would run. */
export function SessionMenu({ session }: { session: string }) {
  const canQuery3 = useQuerySupport(["list_sessions"])
  const canQuery3_2 = useQuerySupport(["list_jobs"])
  const canQuery3_3 = useQuerySupport(["get_approvals"])
  const canQuery3_4 = useQuerySupport(["get_communication"])
  const api = useApi()
  const navigate = useNavigate()
  const parent = useSessionField(session, 'parent')
  const undone = useSessionField(session, 'undone') ?? 0
  const liveTags = useSessionField(session, 'tags')
  // Tags: live from `TitleChanged`, else the stored set from the session list; the owner row (68 §5) is always the list's.
  const sessions = useQuery({ enabled: canQuery3, queryKey: queryKeys.sessions, queryFn: () => api.list_sessions() })
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
  const jobs = useQuery({ queryKey: queryKeys.jobs(session), queryFn: () => api.list_jobs(session), enabled: canQuery3_2 && (open && here && !!session) })
  // 181 D1 / 183 §12.2: a session owned elsewhere moves from its owner (forwarded), so its own engine is no target.
  const targets = moveTargets(mesh.data, here ? (jobs.data ?? []).filter((j) => j.state === 'running').length : 0).filter((t) => here || t.id !== row?.owner)
  const blocked = moveBlocked(row, sessions.data ?? [], agents.data ?? [])
  const queryClient = useQueryClient()
  // 183 §12.10: Share with contact… opens outside the menu, which unmounts its items when it closes.
  const [share, setShare] = useState(false)
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

  // 183 §12.2: the settings the owner applies, read through it (forwarded readers) while the menu is open; a reader
  // answering -32010, an unreachable owner or a parked call grey the setters (§4.4).
  const owner = !here && row ? engineLabel(row.owner, mesh.data?.peers) : undefined
  const approvals = useQuery({ queryKey: ['approvals', session || null], queryFn: () => api.get_approvals(session || null), enabled: canQuery3_3 && open, retry: false })
  const messaging = useQuery({ queryKey: queryKeys.communication(session || null), queryFn: () => api.get_communication(session || null), enabled: canQuery3_4 && open, retry: false })
  const pending = useDrafts((s) => s.settings)
  const pick = useDrafts((s) => s.pick)
  const parked = useOwnerOffline((s) => s.by[session] !== undefined)
  const unreachable = mesh.data?.peers.find((p) => p.id === row?.owner)?.reachable === false
  const offline = parked || unreachable || !!ownerUnreachable(approvals.error) || !!ownerUnreachable(messaging.error)
  // A move is the owner's act (forwarded): with the owner gone, Continue here is the way to take it.
  const moveWhy = blocked ?? (owner !== undefined && offline ? `${owner} is offline — Continue here first` : null)
  const models = useModels(session, open)
  const model = useSessionField(session, 'model')
  const effort = useSessionField(session, 'effort')
  const current = models.find((m) => m.id === model)
  const rows = settingsRows({
    owner,
    self: mesh.data?.self_name || 'this machine',
    offline,
    oldOwner: owner !== undefined && approvals.error instanceof RpcError && approvals.error.code === -32601,
    approvals: !session && approvals.data ? { ...approvals.data, pin: pending.approvals ?? undefined } : approvals.data,
    messaging: !session && messaging.data ? { effective: pending.communication ?? messaging.data.effective } : messaging.data,
    model: current?.name ?? (session ? model : pick.model) ?? 'default',
    effort: session ? effort : pick.effort,
  }).filter((r) => !!session || (r.id !== 'continue' && r.id !== 'move' && r.id !== 'separator'))
  /** A setter through the owner: -32010 parks it behind the OwnerOfflineCard (nothing runs until Continue here). */
  const set = (call: () => Promise<unknown>) =>
    void routed(session, call)
      .then(() => {
        void queryClient.invalidateQueries({ queryKey: ['approvals', session] })
        void queryClient.invalidateQueries({ queryKey: ['communication'] })
      })
      .catch((e: unknown) => toast.error(e instanceof Error ? e.message : String(e)))
  const setApprovals = (v: string) => {
    if (!session) return useDrafts.getState().setSettings({ approvals: v === 'default' ? null : v === 'on' })
    set(() => (v === 'default' ? api.set_approvals(session, approvals.data?.global ?? true, true) : api.set_approvals(session, v === 'on')))
  }

  const focused = usePanes((s) => s.focused === session)
  /** Desktop: beside the chat as a tile; the sheet stays for phones and chats outside the pane tree. */
  const show = (s: 'prompt' | 'jobs') => {
    if (!tileable(session)) setSheet(s)
    else if (s === 'prompt') openPromptTile(session)
    else openJobsTile(session)
  }
  useEffect(() => {
    if (!focused) return
    const on = (e: Event) => {
      const s = surfaceOf(e)
      if (s === 'prompt' || s === 'jobs') show(s)
    }
    window.addEventListener('theuth:open', on)
    return () => window.removeEventListener('theuth:open', on)
  }, [focused, session])

  const ctx: Ctx = {
    api,
    session: session || null,
    parent,
    navigate,
    async reload() {
      useSessions
        .getState()
        .loadHistory(session, await api.load_session(session, null, 60), 'replace')
    },
  }
  const cmd = (line: string) => () =>
    void run(line, ctx).catch((e: unknown) => {
      // 183 §4.4: a routed command (compact, undo…) the owner could not take waits behind the card.
      if (!parkIfOffline(session, e, () => run(line, ctx))) toast.error(e instanceof Error ? e.message : String(e))
    })

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
        <DropdownMenuContent align="end" className="w-72 max-w-[calc(100vw-1rem)]">
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
          {rows.map((r) => (
            <Fragment key={r.id}>
              {r.id === 'header' && <DropdownMenuLabel className="text-[11px] font-normal text-fg-tertiary">{r.label}</DropdownMenuLabel>}
              {r.id === 'offline' && (
                <DropdownMenuItem onSelect={cmd('/take')} className="text-[12px] text-warning">
                  {r.label}
                </DropdownMenuItem>
              )}
              {r.id === 'approvals' && (
                <DropdownMenuSub>
                  <DropdownMenuSubTrigger disabled={r.disabled} className="data-[disabled]:pointer-events-none data-[disabled]:opacity-50">
                    <span className="min-w-0 flex-1">{r.label}</span>
                    <span className="text-xs text-fg-tertiary">{r.note ?? r.options.find((o) => o.value === r.value)?.label}</span>
                  </DropdownMenuSubTrigger>
                  <DropdownMenuSubContent className="w-56">
                    <DropdownMenuRadioGroup value={r.value} onValueChange={setApprovals}>
                      {r.options.map((o) => (
                        <DropdownMenuRadioItem key={o.value} value={o.value}>
                          {o.label}
                        </DropdownMenuRadioItem>
                      ))}
                    </DropdownMenuRadioGroup>
                  </DropdownMenuSubContent>
                </DropdownMenuSub>
              )}
              {r.id === 'messaging' && (
                <DropdownMenuCheckboxItem checked={r.checked} disabled={r.disabled} onCheckedChange={(on) => session ? set(() => api.set_communication(session, on)) : useDrafts.getState().setSettings({ communication: on })}>
                  {r.label}
                </DropdownMenuCheckboxItem>
              )}
              {r.id === 'model' && (
                <DropdownMenuItem disabled={r.disabled} onSelect={() => openSurface('model')}>
                  {r.label}
                  <span className="ml-auto text-fg-tertiary">›</span>
                </DropdownMenuItem>
              )}
              {r.id === 'separator' && <DropdownMenuSeparator />}
              {r.id === 'continue' && (
                <DropdownMenuItem disabled={r.disabled} title={r.disabled ? 'This session already runs on this node' : undefined} onSelect={cmd('/take')}>
                  {r.label}
                </DropdownMenuItem>
              )}
              {r.id === 'move' && targets.length > 0 && moveWhy && (
                <DropdownMenuItem disabled className="flex-col items-start gap-0">
                  Move to
                  <span className="text-[11px] text-fg-tertiary">{moveWhy}</span>
                </DropdownMenuItem>
              )}
              {r.id === 'move' && targets.length > 0 && !moveWhy && (
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
            </Fragment>
          ))}
          <DropdownMenuSeparator />
          {session ? <>
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
          <DropdownMenuItem onSelect={() => show('prompt')}>Show prompt</DropdownMenuItem>
          <DropdownMenuItem onSelect={() => show('jobs')}>Jobs</DropdownMenuItem>
          </> : <DropdownMenuItem onSelect={() => openSurface('memory')}>Memory…</DropdownMenuItem>}
          <DropdownMenuItem onSelect={() => useMessagingSheet.getState().show(session)}>Messaging…</DropdownMenuItem>
          <DropdownMenuItem onSelect={() => useToolsSheet.getState().show(session)}>Tools…</DropdownMenuItem>
          {/* 183 §12.10, lifted 2026-11-17: wherever the connected engine offers the share methods, phone and iPad included. */}
          {session && <Capability required={shareRequirements} hide><SharingItems session={session} onShare={() => setShare(true)} /></Capability>}
        </DropdownMenuContent>
      </DropdownMenu>
      {session && <RenameDialog session={session} open={rename} onOpenChange={setRename} />}
      {share && <ShareDialog session={session} title={row ? chatName(row) : 'this chat'} owner={owner} onClose={() => setShare(false)} />}
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
