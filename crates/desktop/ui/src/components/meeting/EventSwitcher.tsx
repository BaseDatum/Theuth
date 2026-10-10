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
import { type Target, calendarKeys, candidateWindow, choices, eventLine, setEvent } from '@/lib/calendar'
import { queryKeys, useApi } from '@/lib/rpc'
import type { Detection } from '@/rpc/bindings/Detection'
import type { EventRef } from '@/rpc/bindings/EventRef'
import type { MeetingId } from '@/rpc/bindings/MeetingId'

export const ASK = 'Which meeting is this?'

export type EventSwitcherProps =
  /** Before start: the current prompt; `onChanged` gets the engine's re-matched prompt after a pick. */
  | { detection: Detection; onChanged?: (next: Detection | null) => void; className?: string }
  /** After start: the meeting, its manifest event and `started_at` (the candidate window's centre). */
  | { meeting: MeetingId; event: EventRef | null; startedAt: string; className?: string }

export function EventSwitcher(props: EventSwitcherProps) {
  const api = useApi()
  const target: Target = 'detection' in props ? { detection: props.detection } : { meeting: props.meeting }
  const before = 'detection' in props ? props : null
  const current = 'detection' in props ? (props.detection.event?.event ?? null) : props.event
  // The window is fixed per mount: a live `Date.now()` key would refetch on every render.
  const [at] = useState(() => ('startedAt' in props ? Date.parse(props.startedAt) : Date.now()))
  const [from, to] = candidateWindow(at)
  const candidates = useQuery({ queryKey: calendarKeys.events(from, to), queryFn: () => api.calendar_events(from, to) })
  const pick = useAction((id: string | null) => setEvent(api, target, id), {
    keys: 'meeting' in target ? [queryKeys.meeting(target.meeting), queryKeys.meetingStatus] : [queryKeys.meetingStatus],
    onSuccess: (result) => before?.onChanged?.(result as Detection | null),
  })
  const rows = choices(before?.detection.event, candidates.data ?? [], current)

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="sm" className={props.className} disabled={pick.isPending} aria-label={ASK}>
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
          {before ? 'None of these' : 'No event'}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
