// 03 §3: `/meetings` — the recorded meetings (date, title, duration, state, process) and *Start meeting…*.
// Every device (phones over the relay included) reads meetings, summaries and transcripts; only a desktop
// shell talking to its own daemon records — the loopback and the mic are this machine's (58 §6.1).

import { useQuery } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import { AppWindow, MoreHorizontal, Paperclip, Trash2 } from 'lucide-react'
import { type ReactNode, useState } from 'react'
import { dragSource } from '@/components/chat/AttachChip'
import { SectionBand } from '@/components/nav/SectionBand'
import { SectionRow } from '@/components/nav/SectionRow'
import { Column } from '@/components/library/Page'
import { ChatPicker } from '@/components/notes/ChatPicker'
import { Empty, errorText, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { isMacDesktop, touch } from '@/lib/hotkeys'
import { isDesktop } from '@/lib/platform'
import { canPopout, isPopout, openPopout } from '@/lib/popout'
import { queryKeys, useApi } from '@/lib/rpc'
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

/**
 * What every meetings page starts with. In the shell (97b): the Library band, then a 44 px row with `title` — the
 * Meetings · Briefs sub-tabs on the list pages, a text title on the start form and the control center — and the
 * actions (`children`); `back` adds a chevron to `/meetings`. A popped window (`PoppedMeetingNote`, a `?window=`
 * meeting) has no sidebar and a normal title bar, so it keeps the 52 px drag band with no toggle and no inset.
 */
export function MeetingHeader({ title, back, children }: { title: ReactNode; back?: boolean; children?: ReactNode }) {
  const mac = isMacDesktop()
  if (isPopout()) {
    return (
      <header data-tauri-drag-region={mac || undefined} className="flex h-[52px] shrink-0 items-center gap-1 px-3">
        <h2 className="flex flex-1 items-center gap-1 truncate text-sm font-semibold">{title}</h2>
        {children}
      </header>
    )
  }
  return (
    <>
      <SectionBand group="library" />
      <SectionRow back={back ? { to: '/meetings', label: 'Back to meetings' } : undefined} title={title}>
        {children}
      </SectionRow>
    </>
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

/** 05 §7: briefs are a sub-feature of meetings — a tab beside the list, not a sidebar entry. 97b: the sub-tabs sit in the row under the Library band. */
export function MeetingTabs() {
  const tab = 'rounded-md px-2 py-1 text-fg-tertiary hover:text-foreground'
  const active = { className: 'bg-layer-2 text-foreground' }
  return (
    <span className="flex items-center gap-1">
      <Link to="/meetings" activeOptions={{ exact: true }} className={tab} activeProps={active}>
        Meetings
      </Link>
      <Link to="/meetings/briefs" className={tab} activeProps={active}>
        Briefs
      </Link>
    </span>
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
      {/* 143 D5: the same centred column as Tools/Secrets/Notes. */}
      <Column>
        {list.isError && <Empty>{errorText(list.error)}</Empty>}
        {list.data?.length === 0 && <Empty>No meetings yet.</Empty>}
        <ul className="divide-y divide-border-subtle">
          {list.data?.map((m) => (
            <Row key={m.id} m={m} onAttach={() => setAttaching(toRef({ kind: 'meeting', meeting: m }))} />
          ))}
        </ul>
      </Column>
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

/** One meeting: click opens it; hover shows attach · note in new window · delete; right-click (or ⋯ on touch) the same as a menu. No "new window" where `canPopout()` is false.
 * Under `md` the row is two lines (title · `date · process · duration · state`) and only the ⋯ renders, so the icons never sit on the text in a phone-width column. */
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
        <Button variant="ghost" size="icon-sm" aria-label="Attach to chat…" className={`hidden md:inline-flex ${hover}`} onClick={onAttach}>
          <Paperclip />
        </Button>
        {popout && (
          <Button variant="ghost" size="icon-sm" aria-label="Open note in new window" className={`hidden md:inline-flex ${hover}`} onClick={note}>
            <AppWindow />
          </Button>
        )}
        <Button variant="ghost" size="icon-sm" aria-label="Delete meeting" className={`hidden md:inline-flex ${hover}`} disabled={del.isPending} onClick={() => del.mutate()}>
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
        className="flex w-full flex-col gap-0.5 py-3 pr-12 text-left text-sm hover:bg-accent/50 md:flex-row md:items-center md:gap-4 md:pr-32"
        onClick={() => void navigate({ to: '/meetings/$id', params: { id: m.id } })}
        onContextMenu={(e) => {
          if (touch()) return
          e.preventDefault()
          setMenu(true)
        }}
        {...dragSource(() => toRef({ kind: 'meeting', meeting: m }))}
      >
        <span className="hidden w-36 shrink-0 text-fg-secondary tabular-nums md:inline">{new Date(m.started_at).toLocaleString()}</span>
        <span className="min-w-0 flex-1 truncate">{m.title ?? `Meeting with ${m.process_name}`}</span>
        <span className="truncate text-xs text-fg-tertiary md:hidden">
          {[new Date(m.started_at).toLocaleString(), m.process_name, m.duration_ms === null ? null : duration(Number(m.duration_ms)), stateLabel(m.state)]
            .filter((s) => s !== null)
            .join(' · ')}
        </span>
        <span className="hidden shrink-0 text-fg-tertiary md:inline">{m.process_name}</span>
        <span className="hidden w-20 shrink-0 text-right text-fg-secondary tabular-nums md:inline">
          {m.duration_ms === null ? '—' : duration(Number(m.duration_ms))}
        </span>
        <span className="hidden w-24 shrink-0 text-right text-fg-tertiary md:inline">{stateLabel(m.state)}</span>
      </button>
    </li>
  )
}
