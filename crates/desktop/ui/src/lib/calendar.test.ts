import { expect, test, vi } from 'vitest'
import type { CalEvent } from "@/rpc/bindings/method.meeting.output"
import type { EventRef } from "@/rpc/bindings/method.meeting.output"
import { choices, eventLine, eventkitState, setEvent } from './calendar'

const NOW = Date.parse('2026-10-10T10:02:00')

const ref = (id: string, title: string, start: string, end: string): EventRef => ({ id, uid: id, title, start, end, join_url: null, join_host: null, organizer: null, attendees: [], description: null })
const cal = (id: string, title: string, start: string, end: string, extra: Partial<CalEvent> = {}): CalEvent => ({ ...ref(id, title, start, end), source: 's', all_day: false, tz: null, cancelled: false, sequence: 0, location: null, recurrence_id: null, ...extra })

test('event lines read `Title · 10:00–10:30` in local time; tomorrow carries the day', () => {
  expect(eventLine(ref('a', 'Weekly sync', '2026-10-10T10:00:00', '2026-10-10T10:30:00'), NOW)).toBe('Weekly sync · 10:00–10:30')
  expect(eventLine(ref('a', '1:1', '2026-10-11T09:00:00', '2026-10-11T09:45:00'), NOW)).toBe('1:1 · Sun 09:00–09:45')
})

test('choices: match first (shown event, then alternatives), then the window minus cancelled / all-day, each id once; the manifest event stays on top after start', () => {
  const sync = ref('s', 'Weekly sync', '2026-10-10T10:00:00', '2026-10-10T10:30:00')
  const design = ref('d', 'Design review', '2026-10-10T10:00:00', '2026-10-10T10:30:00')
  const window = [
    cal('s', 'Weekly sync', '2026-10-10T10:00:00', '2026-10-10T10:30:00'),
    cal('x', 'Cancelled', '2026-10-10T10:00:00', '2026-10-10T10:30:00', { cancelled: true }),
    cal('h', 'Holiday', '2026-10-10T00:00:00', '2026-10-11T00:00:00', { all_day: true }),
    cal('o', 'Standup', '2026-10-10T09:45:00', '2026-10-10T10:00:00'),
  ]
  expect(choices({ event: design, score: 1.8, alternatives: [sync] }, window, null, NOW).map((c) => c.id)).toEqual(['d', 's', 'o'])
  expect(choices(null, window, null, NOW).map((c) => c.line)).toEqual(['Weekly sync · 10:00–10:30', 'Standup · 09:45–10:00'])
  const gone = ref('g', 'Moved', '2026-10-10T08:00:00', '2026-10-10T08:30:00')
  expect(choices(null, window, gone, NOW).map((c) => c.id)).toEqual(['g', 's', 'o'])
})

test('setEvent routes: no meeting (before start) → meeting_detection_set_event(event); a meeting id → meeting_set_event(id, event); null clears either', async () => {
  const api = { meeting_detection_set_event: vi.fn(async () => null), meeting_set_event: vi.fn(async () => ({}) as never) }
  await setEvent(api, null, 's:uid:')
  await setEvent(api, null, null)
  expect(api.meeting_detection_set_event.mock.calls).toEqual([['s:uid:'], [null]])
  expect(api.meeting_set_event).not.toHaveBeenCalled()
  await setEvent(api, 'm1', 'd:uid:')
  await setEvent(api, 'm1', null)
  expect(api.meeting_set_event.mock.calls).toEqual([
    ['m1', 'd:uid:'],
    ['m1', null],
  ])
  expect(api.meeting_detection_set_event).toHaveBeenCalledTimes(2)
})

test('the macOS Calendar state line: ICS hint off-Mac, companion states on Mac', () => {
  const src = { id: 'eventkit', kind: 'eventkit', name: 'macOS Calendar', enabled: true, last_fetch: null, last_error: null, state: 'granted' }
  expect(eventkitState(true, false, src)).toMatch(/available on Mac/)
  expect(eventkitState(false, true, src)).toMatch(/^Off/)
  expect(eventkitState(true, true, src)).toBe('Access granted')
  expect(eventkitState(true, true, { ...src, state: 'denied' })).toMatch(/System Settings › Privacy › Calendars/)
  expect(eventkitState(true, true, undefined)).toBe('Companion not running')
})
