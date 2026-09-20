// 53 / 03 §9: while a meeting records, every header band's right end carries its controls — clock,
// pause/resume, stop, and a jump to the control center — so stopping never means finding the meeting
// first. Inline (it pushes the header's own controls left), `null` when nothing records. Fed by
// `store/meeting.ts`; the island shows the same clock.

import { useNavigate } from '@tanstack/react-router'
import { Pause, Play, Square } from 'lucide-react'
import { useEffect, useState } from 'react'
import { Button } from '@/components/ui/button'
import { queryKeys, useApi } from '@/lib/rpc'
import { useQueryClient } from '@tanstack/react-query'
import { elapsed, live, useMeeting } from '@/store/meeting'

const mmss = (ms: number): string => {
  const s = Math.floor(ms / 1000)
  return `${String(Math.floor(s / 60)).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}`
}

export function MeetingPill() {
  const api = useApi()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const state = useMeeting((s) => s.state)
  const meeting = useMeeting((s) => s.meeting)
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
  return (
    <div className="ml-1 flex h-8 shrink-0 items-center gap-0.5 rounded-full border border-border-subtle bg-layer-2 pl-3 pr-1 text-xs">
      <button
        type="button"
        className="flex items-center gap-1.5 font-mono tabular-nums hover:underline"
        title={activity ? `Meeting session: ${activity}` : 'Open the meeting'}
        onClick={() => void navigate({ to: '/meetings/$id', params: { id: meeting } })}
      >
        <span className={state === 'Paused' ? 'size-2 rounded-full bg-fg-tertiary' : 'size-2 animate-pulse rounded-full bg-danger'} />
        {mmss(elapsed(useMeeting.getState(), now))}
      </button>
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
