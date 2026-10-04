// 03 §3: `/meetings` — the recorded meetings (date, title, duration, state, process) and *Start meeting…*.
// Every device (phones over the relay included) reads meetings, summaries and transcripts; only a desktop
// shell talking to its own daemon records — the loopback and the mic are this machine's (58 §6.1).

import { useQuery } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import { AppWindow, MoreHorizontal, PanelLeft, Paperclip, Trash2 } from 'lucide-react'
import { type ReactNode, useState } from 'react'
import { dragSource } from '@/components/chat/AttachChip'
import { SectionTabs } from '@/components/nav/SectionTabs'
import { ChatPicker } from '@/components/notes/ChatPicker'
import { Empty, errorText, useAction } from '@/components/settings/shared'
import { SearchTrigger } from '@/components/sidebar/SearchTrigger'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { useSidebar } from '@/components/ui/sidebar'
import { isMacDesktop, touch } from '@/lib/hotkeys'
import { isDesktop } from '@/lib/platform'
import { canPopout, isPopout, openPopout } from '@/lib/popout'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import { LOCAL, useEngines } from '@/link/engines'
import { type AttachRef, toRef } from '@/lib/attach'
import type { Api } from '@/rpc/api'
import type { MeetingState } from '@/rpc/bindings/MeetingState'
import type { MeetingSummary } from '@/rpc/bindings/MeetingSummary'

/** `Post(Transcribing)` → `Transcribing`. */
export const stateLabel = (s: MeetingState): string => (typeof s === 'string' ? s : s.Post)

export const duration = (ms: number): string => {
  const m = Math.round(ms / 60_000)
  return m < 60 ? `${m} min` : `${Math.floor(m / 60)} h ${m % 60} min`
}

/** The 52 px drag band every meetings page starts with: sidebar toggle when the rail is closed, a title (or the tab strip), actions.
 * A popped window has no sidebar and a normal title bar: no toggle, no traffic-light inset. */
export function MeetingHeader({ title, children }: { title: ReactNode; children?: ReactNode }) {
  const sidebar = useSidebar()
  const mac = isMacDesktop()
  const popped = isPopout()
  return (
    <header
      data-tauri-drag-region={mac || undefined}
      className={cn('flex h-[52px] shrink-0 items-center gap-1 px-3', !sidebar.open && mac && !popped && 'pl-20')}
    >
      {(!sidebar.open || sidebar.isMobile) && !popped && (
        <div className="flex shrink-0 items-center">
          <SearchTrigger />
          <Button variant="ghost" size="icon-sm" aria-label="Show sidebar" onClick={sidebar.toggleSidebar}>
            <PanelLeft />
          </Button>
        </div>
      )}
      <h2 className="flex flex-1 items-center gap-1 truncate text-sm font-semibold">{title}</h2>
      {children}
    </header>
  )
}

/**
 * Recording is the local desktop daemon's: not iOS, not a browser, not an engine reached over the relay — and only
 * when its capture backend answers (`MeetingStatus.supported`, 58 §3.3; `None` = no PipeWire / no render device).
 */
export function useCanRecord(): boolean {
  const api = useApi()
  const local = useEngines((s) => s.active) === LOCAL && isDesktop()
  const status = useQuery({ queryKey: queryKeys.meetingStatus, queryFn: () => api.meeting_status(), enabled: local })
  return local && status.data?.supported.kind !== 'None'
}

/** 05 §7: briefs are a sub-feature of meetings — a tab beside the list, not a sidebar entry. */
export function MeetingTabs() {
  const tab = 'rounded-md px-2 py-1 text-fg-tertiary hover:text-foreground'
  const active = { className: 'bg-layer-2 text-foreground' }
  return (
    <>
      <Link to="/meetings" activeOptions={{ exact: true }} className={tab} activeProps={active}>
        Meetings
      </Link>
      <Link to="/meetings/briefs" className={tab} activeProps={active}>
        Briefs
      </Link>
    </>
  )
}

export function MeetingList() {
  const api = useApi()
  const list = useQuery({ queryKey: queryKeys.meetings, queryFn: () => api.meeting_list(null) })
  const canRecord = useCanRecord()
  // 66 §7.8: a row's paperclip → "Attach to chat…"; rows drag onto a composer.
  const [attaching, setAttaching] = useState<AttachRef | null>(null)
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {attaching && <ChatPicker open onOpenChange={(o) => !o && setAttaching(null)} item={attaching} />}
      <MeetingHeader title={<MeetingTabs />}>
        {canRecord && (
          <Button size="sm" asChild>
            <Link to="/meetings/new">Start meeting…</Link>
          </Button>
        )}
      </MeetingHeader>
      <SectionTabs group="library" />
      <div className="min-h-0 flex-1 overflow-y-auto px-6 pb-10">
        {list.isError && <Empty>{errorText(list.error)}</Empty>}
        {list.data?.length === 0 && <Empty>No meetings yet.</Empty>}
        <ul className="divide-y divide-border-subtle">
          {list.data?.map((m) => (
            <Row key={m.id} m={m} onAttach={() => setAttaching(toRef({ kind: 'meeting', meeting: m }))} />
          ))}
        </ul>
      </div>
    </div>
  )
}

/** The list row's and the summary block's Delete: one confirm, one RPC (the engine drops the recording, transcript,
 * summary, session and note, then publishes `Deleted`, which refreshes the list); `false` when declined. */
export async function deleteMeeting(api: Pick<Api, 'meeting_delete'>, id: string): Promise<boolean> {
  if (!confirm('Delete this meeting and its recording?')) return false
  await api.meeting_delete(id)
  return true
}

/** One meeting: click opens it; hover shows attach · note in new window · delete; right-click (or ⋯ on touch) the same as a menu. No "new window" where `canPopout()` is false. */
function Row({ m, onAttach }: { m: MeetingSummary; onAttach: () => void }) {
  const api = useApi()
  const navigate = useNavigate()
  const [menu, setMenu] = useState(false)
  const del = useAction(() => deleteMeeting(api, m.id), { keys: [queryKeys.meetings] })
  const popout = canPopout()
  const note = () => void openPopout({ kind: 'meeting-note', meeting: m.id })
  const hover = 'opacity-0 group-hover/row:opacity-100 focus-visible:opacity-100 data-[state=open]:opacity-100 pointer-coarse:opacity-100'
  return (
    <li className="group/row relative">
      <div className="absolute top-1/2 right-0 flex -translate-y-1/2 items-center">
        <Button variant="ghost" size="icon-sm" aria-label="Attach to chat…" className={hover} onClick={onAttach}>
          <Paperclip />
        </Button>
        {popout && (
          <Button variant="ghost" size="icon-sm" aria-label="Open note in new window" className={hover} onClick={note}>
            <AppWindow />
          </Button>
        )}
        <Button variant="ghost" size="icon-sm" aria-label="Delete meeting" className={hover} disabled={del.isPending} onClick={() => del.mutate()}>
          <Trash2 />
        </Button>
        <DropdownMenu open={menu} onOpenChange={setMenu}>
          <DropdownMenuTrigger asChild>
            <Button variant="ghost" size="icon-sm" aria-label="Meeting options" className={hover}>
              <MoreHorizontal />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuItem onSelect={() => void navigate({ to: '/meetings/$id', params: { id: m.id } })}>Open</DropdownMenuItem>
            {popout && (
              <>
                <DropdownMenuItem onSelect={() => void openPopout({ kind: 'meeting', meeting: m.id })}>Open in new window</DropdownMenuItem>
                <DropdownMenuItem onSelect={note}>Open note in new window</DropdownMenuItem>
              </>
            )}
            <DropdownMenuItem onSelect={onAttach}>Attach to chat…</DropdownMenuItem>
            <DropdownMenuItem variant="destructive" disabled={del.isPending} onSelect={() => del.mutate()}>
              Delete
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
      <button
        type="button"
        className="flex w-full items-center gap-4 py-3 pr-32 text-left text-sm hover:bg-accent/50"
        onClick={() => void navigate({ to: '/meetings/$id', params: { id: m.id } })}
        onContextMenu={(e) => {
          if (touch()) return
          e.preventDefault()
          setMenu(true)
        }}
        {...dragSource(() => toRef({ kind: 'meeting', meeting: m }))}
      >
        <span className="w-36 shrink-0 text-fg-secondary tabular-nums">{new Date(m.started_at).toLocaleString()}</span>
        <span className="min-w-0 flex-1 truncate">{m.title ?? `Meeting with ${m.process_name}`}</span>
        <span className="shrink-0 text-fg-tertiary">{m.process_name}</span>
        <span className="w-20 shrink-0 text-right text-fg-secondary tabular-nums">
          {m.duration_ms === null ? '—' : duration(Number(m.duration_ms))}
        </span>
        <span className="w-24 shrink-0 text-right text-fg-tertiary">{stateLabel(m.state)}</span>
      </button>
    </li>
  )
}
