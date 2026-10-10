/**
 * 158 §7.6 / §8.2 — pure helpers behind Settings › Calendar and the event switcher: event lines, the EventKit
 * state line, the switcher's choice list and which RPC a pick goes to (before start: the detection; after
 * start: the meeting). No React; `calendar.test.ts` covers the tables.
 */
import { formatAt } from '@/lib/time'
import type { CalEvent } from '@/rpc/bindings/CalEvent'
import type { CalSource } from '@/rpc/bindings/CalSource'
import type { Detection } from '@/rpc/bindings/Detection'
import type { EventMatch } from '@/rpc/bindings/EventMatch'
import type { EventRef } from '@/rpc/bindings/EventRef'
import type { Manifest } from '@/rpc/bindings/Manifest'
import type { MeetingId } from '@/rpc/bindings/MeetingId'

const MIN = 60_000

/** React Query keys of the calendar side (not in `queryKeys`: W2U owns no line of `lib/rpc.ts`). */
export const calendarKeys = {
  sources: ['calendar-sources'] as const,
  settings: ['calendar-settings'] as const,
  events: (from: string, to: string) => ['calendar-events', from, to] as const,
}

/** `10:00–10:30` (local); the start carries `formatAt`'s day prefix when it is not today. */
export function eventSpan(start: string, end: string, now = Date.now()): string {
  const e = new Date(end)
  return `${formatAt(Date.parse(start), now)}–${String(e.getHours()).padStart(2, '0')}:${String(e.getMinutes()).padStart(2, '0')}`
}

/** `Weekly sync · 10:00–10:30`. */
export const eventLine = (e: { title: string; start: string; end: string }, now = Date.now()): string => `${e.title} · ${eventSpan(e.start, e.end, now)}`

/** The §8.1 candidate window around `at` as `calendar_events` bounds: events ending ≥ 15 min ago and starting ≤ 10 min ahead. */
export function candidateWindow(at: number): [string, string] {
  return [new Date(at - 15 * MIN).toISOString(), new Date(at + 10 * MIN).toISOString()]
}

/** The macOS Calendar state line (§7.2, §7.6) from the switch and the companion-reported `eventkit` source row. */
export function eventkitState(on: boolean, mac: boolean, source: CalSource | undefined): string {
  if (!mac) return 'macOS Calendar is available on Mac. Add a calendar subscription instead.'
  if (!on) return 'Off. One permission covers every account macOS Calendar knows.'
  switch (source?.state) {
    case 'granted':
      return source?.last_fetch ? `Access granted · pushed ${formatAt(Date.parse(source.last_fetch))}` : 'Access granted'
    case 'denied':
      return 'Calendar access denied — allow theuth Companion in System Settings › Privacy › Calendars'
    default:
      return 'Companion not running'
  }
}

/** One row of the "Which meeting is this?" list. */
export type Choice = { id: string; line: string }

/**
 * The switcher's rows: the match (shown event, then its alternatives, best first), then the window's other
 * candidates (the §8.1 filters: not cancelled, not all-day), each id once. `current` is the manifest's event
 * after start, kept at the top when it is not in the window any more.
 */
export function choices(match: EventMatch | null | undefined, candidates: readonly CalEvent[], current?: EventRef | null, now = Date.now()): Choice[] {
  const seen = new Set<string>()
  const out: Choice[] = []
  const push = (e: { id: string; title: string; start: string; end: string }) => {
    if (seen.has(e.id)) return
    seen.add(e.id)
    out.push({ id: e.id, line: eventLine(e, now) })
  }
  if (current) push(current)
  if (match) {
    push(match.event)
    match.alternatives.forEach(push)
  }
  candidates.filter((e) => !e.cancelled && !e.all_day).forEach(push)
  return out
}

/** The api slice {@link setEvent} needs. */
export type SetEventApi = {
  meeting_detection_set_event(event: string | null): Promise<Detection | null>
  meeting_set_event(id: MeetingId, event: string | null): Promise<Manifest>
}

/** §8.2: before start (`meeting` null) `meeting_detection_set_event`; after start `meeting_set_event(id, …)`. `null` clears. */
export function setEvent(api: SetEventApi, meeting: MeetingId | null, event: string | null): Promise<Detection | Manifest | null> {
  return meeting ? api.meeting_set_event(meeting, event) : api.meeting_detection_set_event(event)
}
