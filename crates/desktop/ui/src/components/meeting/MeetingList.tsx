// 03 §3: `/meetings` — the recorded meetings (date, title, duration, state, process) and *Start meeting…*.
// Every device (phones over the relay included) reads meetings, summaries and transcripts; only a desktop
// shell talking to its own daemon records — the loopback and the mic are this machine's (58 §6.1).

import { useQuery } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import { PanelLeft, Paperclip } from 'lucide-react'
import { type ReactNode, useState } from 'react'
import { dragSource } from '@/components/chat/AttachChip'
import { ChatPicker } from '@/components/notes/ChatPicker'
import { Empty, errorText } from '@/components/settings/shared'
import { SearchTrigger } from '@/components/sidebar/SearchTrigger'
import { Button } from '@/components/ui/button'
import { useSidebar } from '@/components/ui/sidebar'
import { isMacDesktop } from '@/lib/hotkeys'
import { isDesktop } from '@/lib/platform'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import { LOCAL, useEngines } from '@/link/engines'
import { type AttachRef, toRef } from '@/lib/attach'
import type { MeetingState } from '@/rpc/bindings/MeetingState'

/** `Post(Transcribing)` → `Transcribing`. */
export const stateLabel = (s: MeetingState): string => (typeof s === 'string' ? s : s.Post)

export const duration = (ms: number): string => {
  const m = Math.round(ms / 60_000)
  return m < 60 ? `${m} min` : `${Math.floor(m / 60)} h ${m % 60} min`
}

/** The 52 px drag band every meetings page starts with: sidebar toggle when the rail is closed, a title (or the tab strip), actions. */
export function MeetingHeader({ title, children }: { title: ReactNode; children?: ReactNode }) {
  const sidebar = useSidebar()
  const mac = isMacDesktop()
  return (
    <header
      data-tauri-drag-region={mac || undefined}
      className={cn('flex h-[52px] shrink-0 items-center gap-1 px-3', !sidebar.open && mac && 'pl-20')}
    >
      {(!sidebar.open || sidebar.isMobile) && (
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
  const navigate = useNavigate()
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
      <div className="min-h-0 flex-1 overflow-y-auto px-6 pb-10">
        {list.isError && <Empty>{errorText(list.error)}</Empty>}
        {list.data?.length === 0 && <Empty>No meetings yet.</Empty>}
        <ul className="divide-y divide-border-subtle">
          {list.data?.map((m) => (
            <li key={m.id} className="group/row relative">
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label="Attach to chat…"
                className="absolute top-1/2 right-0 -translate-y-1/2 opacity-0 group-hover/row:opacity-100 focus-visible:opacity-100 pointer-coarse:opacity-100"
                onClick={() => setAttaching(toRef({ kind: 'meeting', meeting: m }))}
              >
                <Paperclip />
              </Button>
              <button
                type="button"
                className="flex w-full items-center gap-4 py-3 pr-9 text-left text-sm hover:bg-accent/50"
                onClick={() => void navigate({ to: '/meetings/$id', params: { id: m.id } })}
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
          ))}
        </ul>
      </div>
    </div>
  )
}
