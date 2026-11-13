import { expect, test, vi } from 'vitest'
import { meetingForSession, openMeeting } from './meeting-link'
import type { MeetingSummary } from "@/rpc/bindings/method.meeting.output"

const row = (id: string, session: string): MeetingSummary =>
  ({ id, session, started_at: '', ended_at: null, title: null, process_name: 'Zoom', duration_ms: null, state: 'Done' }) as MeetingSummary

test('a chat gets the meeting items only when a meeting names it as its session', () => {
  const list = [row('m1', 's1'), row('m2', 's2')]
  expect(meetingForSession(list, 's2')).toBe('m2')
  expect(meetingForSession(list, 's3')).toBeNull()
  // The list not loaded yet (or no meeting plugin): hidden, not an error.
  expect(meetingForSession(undefined, 's1')).toBeNull()
  expect(meetingForSession([], 's1')).toBeNull()
})

test('⌥ on the selecting click pops the meeting out; a plain click or a key opens it here', () => {
  const here = vi.fn()
  openMeeting('m1', false, here)
  expect(here).toHaveBeenCalledTimes(1)
  // Not in Tauri here, so the popout is a named `window.open`.
  const open = vi.fn()
  vi.stubGlobal('window', { open })
  openMeeting('m1', true, here)
  expect(here).toHaveBeenCalledTimes(1)
  expect(open).toHaveBeenCalledWith('/meetings/m1?window=popout-meeting-m1', 'popout-meeting-m1')
  // The iOS shell has no second window: ⌥ (a hardware keyboard) is a plain open.
  const invoke = vi.fn()
  vi.stubGlobal('window', { open, __TAURI_INTERNALS__: { invoke } })
  vi.stubGlobal('navigator', { userAgent: 'Mozilla/5.0 (iPhone; CPU iPhone OS 17_4 like Mac OS X)', maxTouchPoints: 5 })
  openMeeting('m1', true, here)
  expect(here).toHaveBeenCalledTimes(2)
  expect(open).toHaveBeenCalledTimes(1)
  expect(invoke).not.toHaveBeenCalled()
  vi.unstubAllGlobals()
})
