// 03 §3: `/meetings` — the recorded meetings (date, title, duration, state, process) and *Start meeting…*.

import { useQuery } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import { PanelLeft } from 'lucide-react'
import type { ReactNode } from 'react'
import { Empty, errorText } from '@/components/settings/shared'
import { SearchTrigger } from '@/components/sidebar/SearchTrigger'
import { Button } from '@/components/ui/button'
import { useSidebar } from '@/components/ui/sidebar'
import { isMacDesktop } from '@/lib/hotkeys'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { MeetingState } from '@/rpc/bindings/MeetingState'
import { MeetingPill } from '@/components/meeting/MeetingPill'

/** `Post(Transcribing)` → `Transcribing`. */
export const stateLabel = (s: MeetingState): string => (typeof s === 'string' ? s : s.Post)

export const duration = (ms: number): string => {
  const m = Math.round(ms / 60_000)
  return m < 60 ? `${m} min` : `${Math.floor(m / 60)} h ${m % 60} min`
}

/** The 52 px drag band every meetings page starts with: sidebar toggle when the rail is closed, a title, actions. */
export function MeetingHeader({ title, children }: { title: string; children?: ReactNode }) {
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
      <h2 className="flex-1 truncate text-sm font-semibold">{title}</h2>
      {children}
      <MeetingPill />
    </header>
  )
}

export function MeetingList() {
  const api = useApi()
  const navigate = useNavigate()
  const list = useQuery({ queryKey: queryKeys.meetings, queryFn: () => api.meeting_list(null) })
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <MeetingHeader title="Meetings">
        <Button size="sm" asChild>
          <Link to="/meetings/new">Start meeting…</Link>
        </Button>
      </MeetingHeader>
      <div className="min-h-0 flex-1 overflow-y-auto px-6 pb-10">
        {list.isError && <Empty>{errorText(list.error)}</Empty>}
        {list.data?.length === 0 && <Empty>No meetings yet.</Empty>}
        <ul className="divide-y divide-border-subtle">
          {list.data?.map((m) => (
            <li key={m.id}>
              <button
                type="button"
                className="flex w-full items-center gap-4 py-3 text-left text-sm hover:bg-accent/50"
                onClick={() => void navigate({ to: '/meetings/$id', params: { id: m.id } })}
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
