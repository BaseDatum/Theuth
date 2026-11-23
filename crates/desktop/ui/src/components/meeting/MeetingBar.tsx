// 53 §4.8 (as built; supersedes 03 §9's per-header pill): while a meeting records, one slim bar across
// the bottom of the routed column — below every pane and composer — carries its controls: clock,
// pause/resume, stop, and the *Meeting* button back to the control center (filled when the open chat
// is the meeting's own session). Mounted once by `routes/__root.tsx`, so pane count and route never
// move it and it never shares a row with a pane's own buttons; `null` when nothing records. Fed by
// `store/meeting.ts`; the island shows the same clock.

import { useNavigate, useParams } from '@tanstack/react-router'
import { useQueryClient } from '@tanstack/react-query'
import { AudioLines, Pause, Play, Square } from 'lucide-react'
import { useEffect, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { queryKeys, useApi } from '@/lib/rpc'
import { mmss } from '@/lib/time-format'
import { elapsed, live, useMeeting } from '@/store/meeting'

export function MeetingBar() {
  const api = useApi()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const { session: open } = useParams({ strict: false })
  const state = useMeeting((s) => s.state)
  const meeting = useMeeting((s) => s.meeting)
  const session = useMeeting((s) => s.session)
  const activity = useMeeting((s) => s.activity)
  const [now, setNow] = useState(Date.now)
  const on = live(state)
  useEffect(() => {
    if (state !== 'Recording') return
    const t = window.setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(t)
  }, [state])
  if (!on || !meeting) return null
  const act = (p: Promise<unknown>) =>
    void p.then(() => qc.invalidateQueries({ queryKey: queryKeys.meetingStatus })).catch(() => undefined)
  const toMeeting = () => void navigate({ to: '/meetings/$id', params: { id: meeting } })
  return (
    <div className="flex h-9 shrink-0 items-center gap-1 border-t border-border-subtle bg-layer-2 px-3 text-xs">
      <button
        type="button"
        className="flex items-center gap-1.5 font-mono tabular-nums hover:underline"
        title={activity ? `Meeting session: ${activity}` : 'Open the meeting'}
        onClick={toMeeting}
      >
        <span className={state === 'Paused' ? 'size-2 rounded-full bg-fg-tertiary' : 'size-2 animate-pulse rounded-full bg-danger'} />
        {mmss(elapsed(useMeeting.getState(), now))}
      </button>
      {activity && <span className="hidden truncate text-fg-tertiary sm:inline">{activity}</span>}
      <div className="flex-1" />
      <Tooltip>
        <TooltipTrigger asChild>
          <Button variant={open !== undefined && open === session ? 'default' : 'outline'} size="xs" aria-label="Open meeting" onClick={toMeeting}>
            <AudioLines />
            <span className="hidden md:inline">Meeting</span>
          </Button>
        </TooltipTrigger>
        <TooltipContent>Back to the meeting</TooltipContent>
      </Tooltip>
      {state === 'Paused' ? (
        <Button variant="ghost" size="icon-xs" aria-label="Resume recording" onClick={() => act(api.meeting_resume())}>
          <Play />
        </Button>
      ) : (
        <Button variant="ghost" size="icon-xs" aria-label="Pause recording" onClick={() => act(api.meeting_pause())}>
          <Pause />
        </Button>
      )}
      <Button variant="ghost" size="icon-xs" aria-label="Stop recording" onClick={() => act(api.meeting_stop())}>
        <Square className="fill-current" />
      </Button>
    </div>
  )
}
