import { useQuerySupport } from '@/rpc/capabilities'
// 158 §5.5 (W1, W-q): the Meetings-page banner — the same prompt the islands show. "<App> is using your
// microphone", the event line + project chip when present, Start recording / Not now / ⋮ Never for <App>,
// and the R3 lawful sentence. After `AppReleased` during a recording: "Stop recording? / Keep recording".
// Seeded from `meeting_detection`, then driven by the bus (`lib/detection.ts` `reduce`). Linux's only surface;
// on macOS/Windows the fallback when the companion is not alive. Only the local desktop daemon detects.

import { useQuery } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { Mic, MicOff, MoreHorizontal } from 'lucide-react'
import { type ReactNode, useEffect, useReducer } from 'react'
import { EventSwitcher } from '@/components/meeting/EventSwitcher'
import { Column } from '@/components/library/Page'
import { useAction } from '@/components/settings/shared'
import { ProjectIcon } from '@/components/sidebar/ProjectStyle'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { EMPTY, LAWFUL, type Released, detectedStart, reduce } from '@/lib/detection'
import { eventLine } from '@/lib/calendar'
import { isDesktop } from '@/lib/platform'
import { queryKeys, useApi } from '@/lib/rpc'
import { LOCAL, useEngines } from '@/link/engines'
import type { Detection } from "@/rpc/bindings/method.meeting.output"
import { on } from '@/store/sessions'

export const DETECTION_KEY = ['meeting-detection'] as const

export function DetectionBanner() {
  const canQuery1 = useQuerySupport(["meeting_detection"])
  const api = useApi()
  const navigate = useNavigate()
  const local = useEngines((s) => s.active) === LOCAL && isDesktop()
  const initial = useQuery({ queryKey: DETECTION_KEY, queryFn: () => api.meeting_detection(), enabled: canQuery1 && (local) })
  const [state, dispatch] = useReducer(reduce, EMPTY)
  useEffect(() => {
    const offs = (['Detected', 'DetectionCleared', 'AppReleased', 'Started', 'State'] as const).map((t) => on("meeting.events@1", t, dispatch))
    return () => {
      for (const off of offs) off()
    }
  }, [])

  const start = useAction((d: Detection) => api.meeting_start(detectedStart(d)), {
    keys: [queryKeys.meetings, queryKeys.meetingStatus, DETECTION_KEY],
    onSuccess: (started) => void navigate({ to: '/meetings/$id', params: { id: started.meeting } }),
  })
  const dismiss = useAction(({ app_id, never }: { app_id: string; never: boolean }) => api.meeting_detection_dismiss(app_id, never), {
    keys: [DETECTION_KEY],
    onSuccess: (_, { app_id, never }) => dispatch({ type: 'DetectionCleared', app_id, reason: never ? 'Never' : 'NotNow' }),
  })
  const stop = useAction((_: void) => api.meeting_stop(), { keys: [queryKeys.meetingStatus] })

  if (!local) return null
  // The RPC answer fills the gap before the first bus event; after that the bus is the truth.
  const detection = state.live ? state.detection : (initial.data ?? null)
  const { released } = state
  if (released) return <ReleasePrompt released={released} pending={stop.isPending} onStop={() => stop.mutate()} onKeep={() => dispatch({ type: 'Keep' })} />
  if (!detection) return null
  const d = detection
  const ev = d.event?.event
  return (
    <Band icon={<Mic className="size-4 text-danger" />}>
      <div className="text-sm font-medium">{d.app.name} is using your microphone</div>
      {(ev || d.project) && (
        <div className="flex flex-wrap items-center gap-2 text-sm text-fg-secondary">
          {ev && <span>{eventLine(ev)}</span>}
          {d.project && (
            <Badge variant="outline">
              <ProjectIcon project={d.project} className="size-3" />
              {d.project}
            </Badge>
          )}
          <EventSwitcher detection={d} />
        </div>
      )}
      <div className="flex items-center gap-2">
        <Button size="sm" disabled={start.isPending} onClick={() => start.mutate(d)}>
          Start recording
        </Button>
        <Button size="sm" variant="outline" disabled={dismiss.isPending} onClick={() => dismiss.mutate({ app_id: d.app_id, never: false })}>
          Not now
        </Button>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button variant="ghost" size="icon-sm" aria-label="More">
              <MoreHorizontal />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start">
            <DropdownMenuItem onSelect={() => dismiss.mutate({ app_id: d.app_id, never: true })}>Never for {d.app.name}</DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
      <p className="text-xs text-fg-tertiary">{LAWFUL}</p>
    </Band>
  )
}

/** §5.2 (d): the app let go while the meeting records; ⏎ is Keep (the safe default), so Stop is never the primary. */
export function ReleasePrompt({ released, pending, onStop, onKeep }: { released: Released; pending?: boolean; onStop(): void; onKeep(): void }) {
  return (
    <Band icon={<MicOff className="size-4 text-fg-tertiary" />}>
      <div className="text-sm">
        {released.app.name} stopped using the microphone{released.released_for_s > 0 ? ` ${released.released_for_s} s ago` : ''} — Stop recording?
      </div>
      <div className="flex gap-2">
        <Button size="sm" variant="destructive" disabled={pending} onClick={onStop}>
          Stop
        </Button>
        <Button size="sm" variant="outline" onClick={onKeep}>
          Keep recording
        </Button>
      </div>
    </Band>
  )
}

function Band({ icon, children }: { icon: ReactNode; children: ReactNode }) {
  return (
    <div className="border-b border-border-subtle bg-layer-2">
      <Column>
        <div className="flex items-start gap-3 py-3">
          <span className="mt-0.5">{icon}</span>
          <div className="flex min-w-0 flex-1 flex-col gap-2">{children}</div>
        </div>
      </Column>
    </div>
  )
}
