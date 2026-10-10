// 158 §8.2 (W2U): the `[▾]` event list. Before start (`detection`) a pick drives `meeting_detection_set_event`
// and the prompt re-renders from the returned `Detection`; after start (`meeting`) the header's Event chip drives
// `meeting_set_event`. The rows are the match (shown event + alternatives) plus the §8.1 window's other
// candidates from `calendar_events`; "None of these" clears.

import { useQuery } from '@tanstack/react-query'
import { ChevronDown } from 'lucide-react'
import { useState } from 'react'
import { errorText, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { calendarKeys, candidateWindow, choices, eventLine, setEvent } from '@/lib/calendar'
import { queryKeys, useApi } from '@/lib/rpc'
import type { Detection } from '@/rpc/bindings/Detection'
import type { MeetingId } from '@/rpc/bindings/MeetingId'

export const ASK = 'Which meeting is this?'

/**
 * Before start: `detection` (W1U's banner passes the prompt; `onChanged` gets the engine's re-matched prompt after a
 * pick). After start: `meeting` — the manifest (`queryKeys.meeting(id)`, shared with the control centre) supplies the
 * current event and `started_at`, the candidate window's centre. `meeting` wins when both are given.
 */
export type EventSwitcherProps = { detection?: Detection; meeting?: MeetingId; onChanged?: (next: Detection | null) => void; className?: string }

export function EventSwitcher({ detection, meeting, onChanged, className }: EventSwitcherProps) {
  const api = useApi()
  const manifest = useQuery({ queryKey: queryKeys.meeting(meeting ?? ''), queryFn: () => api.meeting_get(meeting!), enabled: meeting !== undefined })
  const current = meeting ? (manifest.data?.event ?? null) : (detection?.event?.event ?? null)
  // Before start the window is fixed per mount (a live `Date.now()` key would refetch every render); after start it is the meeting's.
  const [mounted] = useState(() => Date.now())
  const [from, to] = candidateWindow(manifest.data ? Date.parse(manifest.data.started_at) : mounted)
  const candidates = useQuery({ queryKey: calendarKeys.events(from, to), queryFn: () => api.calendar_events(from, to) })
  const pick = useAction((id: string | null) => setEvent(api, meeting ?? null, id), {
    keys: meeting ? [queryKeys.meeting(meeting), queryKeys.meetingStatus] : [queryKeys.meetingStatus],
    onSuccess: (result) => !meeting && onChanged?.(result as Detection | null),
  })
  const rows = choices(meeting ? null : detection?.event, candidates.data ?? [], current)
  if (!detection && !meeting) return null

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="sm" className={className} disabled={pick.isPending} aria-label={ASK}>
          <span className="truncate">{current ? eventLine(current) : ASK}</span>
          <ChevronDown className="size-4" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="max-w-96">
        <DropdownMenuLabel>{ASK}</DropdownMenuLabel>
        {candidates.isError && <DropdownMenuLabel className="text-danger">{errorText(candidates.error)}</DropdownMenuLabel>}
        {rows.length === 0 && !candidates.isLoading && <DropdownMenuLabel className="font-normal text-fg-tertiary">No events around now.</DropdownMenuLabel>}
        <DropdownMenuRadioGroup value={current?.id ?? ''} onValueChange={(id) => pick.mutate(id)}>
          {rows.map((r) => (
            <DropdownMenuRadioItem key={r.id} value={r.id}>
              {r.line}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
        <DropdownMenuSeparator />
        <DropdownMenuItem disabled={current === null} onSelect={() => pick.mutate(null)}>
          {meeting ? 'No event' : 'None of these'}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
